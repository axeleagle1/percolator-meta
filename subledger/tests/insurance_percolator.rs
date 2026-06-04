//! Real-percolator litesvm end-to-end test for the non-custodial insurance
//! deposit / vote / exit flow.
//!
//! Proves, against the REAL percolator binary
//! (`../percolator-prog/target/deploy/percolator_prog.so`, loaded into litesvm):
//!
//! 1. A user deposits into market-0 INSURANCE through the `subledger` program (the
//!    subledger pool PDA is asset-0's insurance authority + operator). Funds land in
//!    the Percolator insurance vault and a subledger position records
//!    `owner, principal, start_slot`.
//! 2. `genesis-vote` reads that subledger position (principal + start_slot) and the
//!    pool's `outstanding_principal` to weight a vote.
//! 3. The user does a principal-only, owner-authorized exit through the subledger
//!    and gets their principal back. Non-owner exits and over-principal exits fail.
//!
//! Market-0 setup note: the percolator `UpdateAssetAuthority` handler requires the
//! *incoming* authority to co-sign when it is non-zero. The subledger pool is a
//! PDA, which cannot co-sign a top-level instruction, so we cannot rotate authority
//! to it with a plain `UpdateAssetAuthority`. Instead — exactly like the existing
//! genesis integration's manual market — we build the Live market-0 slab with the
//! real percolator state helper `init_market_account_zero_copy`, setting
//! `marketauth = pool_pda` (which percolator copies into asset-0's
//! insurance_authority + insurance_operator + asset_admin) and the deposits-only
//! insurance-withdraw policy (max_bps=10000, deposits_only=1, cooldown=0). The real
//! percolator binary then validates every TopUp/Withdraw CPI against that stored
//! state. This is the on-chain equivalent of the production flow, where the market
//! is born under the controlling PDA via a PDA-signed `InitMarket` CPI.

use litesvm::LiteSVM;
use solana_program_runtime::compute_budget::ComputeBudget;
use solana_sdk::{
    account::Account,
    clock::Clock,
    compute_budget::ComputeBudgetInstruction,
    instruction::{AccountMeta, Instruction},
    program_pack::Pack,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};

const ATA_PROGRAM_ID: Pubkey =
    solana_sdk::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

fn sub_id() -> Pubkey {
    subledger_program::id()
}
fn gv_id() -> Pubkey {
    genesis_vote_program::id()
}
fn dist_id() -> Pubkey {
    distribution_program::id()
}
fn perc_id() -> Pubkey {
    percolator_prog::id()
}

fn so(name: &str) -> String {
    format!("{}/../target/deploy/{}.so", env!("CARGO_MANIFEST_DIR"), name)
}
fn perc_so() -> String {
    format!(
        "{}/../../percolator-prog/target/deploy/percolator_prog.so",
        env!("CARGO_MANIFEST_DIR")
    )
}
fn clone_kp(kp: &Keypair) -> Keypair {
    Keypair::from_bytes(&kp.to_bytes()).unwrap()
}

const ASSET_ID: u64 = 0;
const POLICY_PRINCIPAL: u8 = 0;

struct Env {
    svm: LiteSVM,
    payer: Keypair,
    /// The at-risk COLLATERAL mint (mintable here to fund depositors). The subledger
    /// insurance pool and the percolator market-0 collateral use this.
    mint: Pubkey,
    /// The distributed COIN mint — a DIFFERENT, fixed-supply token (mint authority
    /// revoked at distribution init). genesis-vote + distribution are keyed by this.
    coin_mint: Pubkey,
    mint_auth: Keypair,
    slab: Pubkey,
    vault_authority: Pubkey,
    perc_vault: Pubkey,
    pool: Pubkey,
}

impl Env {
    fn new() -> Self {
        let mut svm = LiteSVM::new().with_compute_budget(ComputeBudget {
            compute_unit_limit: 1_400_000,
            heap_size: 256 * 1024,
            ..ComputeBudget::default()
        });
        svm.add_program_from_file(sub_id(), so("subledger_program")).unwrap();
        svm.add_program_from_file(gv_id(), so("genesis_vote_program")).unwrap();
        svm.add_program_from_file(dist_id(), so("distribution_program")).unwrap();
        svm.add_program_from_file(perc_id(), perc_so()).unwrap();

        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
        let mint_auth = Keypair::new();
        let mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
        // The distributed COIN is a separate fixed-supply token (authority revoked in
        // setup_vote once the distribution vault is funded).
        let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());

        // The market slab is chosen first; the pool PDA commits to it (finding Q).
        let slab = Pubkey::new_unique();

        // The subledger insurance pool PDA: asset-0 insurance authority + operator,
        // bound to (mint, asset_id, market_slab, percolator_program).
        let pool = Pubkey::find_program_address(
            &[
                b"subledger_pool",
                mint.as_ref(),
                &ASSET_ID.to_le_bytes(),
                slab.as_ref(),
                perc_id().as_ref(),
            ],
            &sub_id(),
        )
        .0;

        // Build the real Live market-0 slab with marketauth = pool PDA and the
        // deposits-only principal-recovery insurance policy.
        let init_slot = 100u64;
        let slab_data = make_live_market(&slab, &mint, &pool, init_slot);
        svm.set_account(
            slab,
            Account {
                lamports: 1_000_000_000,
                data: slab_data,
                owner: perc_id(),
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();

        let vault_authority =
            Pubkey::find_program_address(&[b"vault", slab.as_ref()], &perc_id()).0;
        // The canonical insurance vault: ATA of vault_authority for `mint`.
        let perc_vault = Pubkey::find_program_address(
            &[vault_authority.as_ref(), spl_token::ID.as_ref(), mint.as_ref()],
            &ATA_PROGRAM_ID,
        )
        .0;
        svm.set_account(
            perc_vault,
            Account {
                lamports: 1_000_000,
                data: token_account_data(&mint, &vault_authority, 0),
                owner: spl_token::ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();

        svm.set_sysvar(&Clock {
            slot: init_slot,
            unix_timestamp: 100,
            ..Clock::default()
        });

        Env {
            svm,
            payer,
            mint,
            coin_mint,
            mint_auth,
            slab,
            vault_authority,
            perc_vault,
            pool,
        }
    }

    fn send(&mut self, ixs: &[Instruction], extra: &[&Keypair]) -> Result<(), String> {
        self.svm.expire_blockhash();
        let bh = self.svm.latest_blockhash();
        let payer = clone_kp(&self.payer);
        let mut signers: Vec<&Keypair> = vec![&payer];
        signers.extend_from_slice(extra);
        let pk = self.payer.pubkey();
        let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(1_400_000)];
        all.extend_from_slice(ixs);
        let tx = Transaction::new_signed_with_payer(&all, Some(&pk), &signers, bh);
        self.svm.send_transaction(tx).map(|_| ()).map_err(|e| format!("{:?}", e))
    }

    fn token_amount(&self, account: &Pubkey) -> u64 {
        let acc = self.svm.get_account(account).unwrap();
        spl_token::state::Account::unpack(&acc.data).unwrap().amount
    }

    fn warp_slot(&mut self, slot: u64) {
        self.svm.set_sysvar(&Clock {
            slot,
            unix_timestamp: slot as i64,
            ..Clock::default()
        });
    }

    fn position_pda(&self, owner: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[b"subledger_position", self.pool.as_ref(), owner.as_ref()],
            &sub_id(),
        )
        .0
    }

    // ---- subledger ----

    fn init_insurance_pool(&mut self) {
        let mut data = vec![3u8]; // IX_INIT_INSURANCE_POOL
        data.extend_from_slice(&ASSET_ID.to_le_bytes());
        data.push(POLICY_PRINCIPAL);
        let ix = Instruction {
            program_id: sub_id(),
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(self.mint, false),
                AccountMeta::new(self.pool, false),
                AccountMeta::new_readonly(self.perc_vault, false),
                AccountMeta::new_readonly(self.slab, false),
                AccountMeta::new_readonly(perc_id(), false),
                AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
                // vote_authority = the genesis-vote config PDA (keyed by the COIN).
                AccountMeta::new_readonly(gv_config_pda(&self.coin_mint, &self.pool), false),
            ],
            data,
        };
        self.send(&[ix], &[]).expect("init insurance pool");
    }

    fn insurance_deposit(
        &mut self,
        owner: &Keypair,
        owner_ata: &Pubkey,
        holding: &Pubkey,
        amount: u64,
    ) -> Result<(), String> {
        let mut data = vec![4u8]; // IX_INSURANCE_DEPOSIT
        data.extend_from_slice(&amount.to_le_bytes());
        let ix = Instruction {
            program_id: sub_id(),
            accounts: vec![
                AccountMeta::new(owner.pubkey(), true),
                AccountMeta::new(self.pool, false),
                AccountMeta::new(self.position_pda(&owner.pubkey()), false),
                AccountMeta::new(*owner_ata, false),
                AccountMeta::new(*holding, false),
                AccountMeta::new(self.slab, false),
                AccountMeta::new(self.perc_vault, false),
                AccountMeta::new_readonly(perc_id(), false),
                AccountMeta::new_readonly(spl_token::ID, false),
                AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            ],
            data,
        };
        self.send(&[ix], &[owner])
    }

    fn insurance_withdraw(
        &mut self,
        owner: &Keypair,
        owner_ata: &Pubkey,
        holding: &Pubkey,
        signer: &Keypair,
        amount: u64,
    ) -> Result<(), String> {
        let mut data = vec![5u8]; // IX_INSURANCE_WITHDRAW
        data.extend_from_slice(&amount.to_le_bytes());
        let ix = Instruction {
            program_id: sub_id(),
            accounts: vec![
                AccountMeta::new(signer.pubkey(), true),
                AccountMeta::new(self.pool, false),
                AccountMeta::new(self.position_pda(&owner.pubkey()), false),
                AccountMeta::new(*owner_ata, false),
                AccountMeta::new(*holding, false),
                AccountMeta::new(self.slab, false),
                AccountMeta::new(self.perc_vault, false),
                AccountMeta::new_readonly(self.vault_authority, false),
                AccountMeta::new_readonly(perc_id(), false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ],
            data,
        };
        self.send(&[ix], &[signer])
    }

    fn read_position(&self, owner: &Pubkey) -> (u64, u64, bool) {
        let acc = self.svm.get_account(&self.position_pda(owner)).unwrap();
        let principal = u64::from_le_bytes(acc.data[72..80].try_into().unwrap());
        let start_slot = u64::from_le_bytes(acc.data[89..97].try_into().unwrap());
        let withdrawn = acc.data[88] == 1;
        (principal, start_slot, withdrawn)
    }

    fn pool_outstanding(&self) -> u64 {
        let acc = self.svm.get_account(&self.pool).unwrap();
        u64::from_le_bytes(acc.data[80..88].try_into().unwrap())
    }
}

// A pool-PDA-owned holding token account (created per depositor).
fn create_holding(env: &mut Env, owner_pool: &Pubkey) -> Pubkey {
    let acc = Keypair::new();
    let rent = env
        .svm
        .minimum_balance_for_rent_exemption(spl_token::state::Account::LEN);
    let mint = env.mint;
    let ixs = [
        system_instruction::create_account(
            &env.payer.pubkey(),
            &acc.pubkey(),
            rent,
            spl_token::state::Account::LEN as u64,
            &spl_token::ID,
        ),
        spl_token::instruction::initialize_account(&spl_token::ID, &acc.pubkey(), &mint, owner_pool)
            .unwrap(),
    ];
    let payer = clone_kp(&env.payer);
    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&payer.pubkey()),
        &[&payer, &acc],
        env.svm.latest_blockhash(),
    );
    env.svm.send_transaction(tx).unwrap();
    acc.pubkey()
}

/// Slab base offset of the percolator `MarketGroupV16` header
/// (HEADER_LEN 16 + WRAPPER_CONFIG_LEN 432).
const MARKET_GROUP_OFF: usize = 448;

/// Drive the live asset-0 insurance down to `new_insurance` *consistently*, exactly as a real
/// venue loss would: insurance, vault, the per-domain budgets and the remaining-budget total
/// all drop together so percolator's `validate_shape` invariants still hold. Every offset is
/// pinned against the REAL percolator struct (`offset_of!`) or canaried by value, so a layout
/// change in the percolator binary fails loudly here instead of silently mis-reading the fund.
fn impair_market(env: &mut Env, new_insurance: u128) {
    use percolator::MarketGroupV16HeaderAccount as H;
    let off_vault = MARKET_GROUP_OFF + core::mem::offset_of!(H, vault);
    let off_ins = MARKET_GROUP_OFF + core::mem::offset_of!(H, insurance);
    let off_rem = MARKET_GROUP_OFF + core::mem::offset_of!(H, insurance_domain_budget_remaining_total);
    // The exact constant the subledger ships must equal offset_of(insurance) — the whole point
    // of the pro-rata feature is reading the insurance fund, NOT the (larger) vault total.
    assert_eq!(off_ins, MARKET_GROUP_OFF + 301, "insurance offset drifted from real percolator struct");
    assert_ne!(off_ins, off_vault, "insurance must not alias vault");

    // The asset-0 domain budgets live in the first asset slot (Market<T>), which the real
    // percolator binary packs immediately after the header. Locate the [long, short] u128 pair
    // by value (both == half the funded insurance after the 50/50 credit split) and canary that
    // they reconcile to the header's remaining-budget total.
    let mut acct = env.svm.get_account(&env.slab).unwrap();
    let rd = |d: &[u8], o: usize| u128::from_le_bytes(d[o..o + 16].try_into().unwrap());
    let rem = rd(&acct.data, off_rem);
    let mut off_long = None;
    let slot0 = MARKET_GROUP_OFF + core::mem::size_of::<H>();
    for o in slot0..acct.data.len().saturating_sub(48) {
        if rd(&acct.data, o) == rem / 2
            && rd(&acct.data, o + 16) == rem - rem / 2
            && rd(&acct.data, o + 32) == 0  // spent_long
            && rd(&acct.data, o + 48) == 0  // spent_short
        {
            off_long = Some(o);
            break;
        }
    }
    let off_long = off_long.expect("locate asset-0 domain budget pair in slab");
    let off_short = off_long + 16;
    assert_eq!(
        rd(&acct.data, off_long) + rd(&acct.data, off_short),
        rem,
        "domain budgets must sum to remaining-budget total (layout canary)"
    );

    let long = new_insurance / 2;
    let short = new_insurance - long;
    acct.data[off_vault..off_vault + 16].copy_from_slice(&new_insurance.to_le_bytes());
    acct.data[off_ins..off_ins + 16].copy_from_slice(&new_insurance.to_le_bytes());
    acct.data[off_rem..off_rem + 16].copy_from_slice(&new_insurance.to_le_bytes());
    acct.data[off_long..off_long + 16].copy_from_slice(&long.to_le_bytes());
    acct.data[off_short..off_short + 16].copy_from_slice(&short.to_le_bytes());
    env.svm.set_account(env.slab, acct).unwrap();
}

fn make_live_market(slab: &Pubkey, mint: &Pubkey, marketauth: &Pubkey, init_slot: u64) -> Vec<u8> {
    let initial_price = 1_000_000u64;
    let mut wrapper = percolator_prog::state::WrapperConfigV16::default();
    wrapper.marketauth = marketauth.to_bytes();
    wrapper.collateral_mint = mint.to_bytes();
    wrapper.last_good_oracle_slot = init_slot;
    // Principal-only insurance withdraw: deposits_only caps to deposited principal,
    // never market profits; max_bps=10000 + cooldown=0 = full principal, no rate limit.
    wrapper.insurance_withdraw_max_bps = 10_000;
    wrapper.insurance_withdraw_deposits_only = 1;
    wrapper.insurance_withdraw_cooldown_slots = 0;
    wrapper.permissionless_resolve_stale_slots = 2_000;
    wrapper.force_close_delay_slots = 100;
    wrapper.oracle_mode = percolator_prog::constants::ORACLE_MODE_MANUAL;
    wrapper.mark_ewma_e6 = initial_price;
    wrapper.mark_ewma_last_slot = init_slot;
    wrapper.mark_ewma_halflife_slots =
        percolator_prog::constants::DEFAULT_MARK_EWMA_HALFLIFE_SLOTS;
    wrapper.oracle_target_price_e6 = initial_price;

    let mut data = vec![0u8; percolator_prog::constants::MARKET_ACCOUNT_LEN];
    let mut cfg = percolator_prog::risk::V16Config::public_user_fund(1, 0, 10);
    cfg.min_nonzero_mm_req = 1;
    cfg.min_nonzero_im_req = 2;
    cfg.maintenance_margin_bps = 10_000;
    cfg.initial_margin_bps = 10_000;
    cfg.max_trading_fee_bps = 10_000;
    cfg.max_accrual_dt_slots = 1;
    cfg.min_funding_lifetime_slots = 1;
    cfg.max_price_move_bps_per_slot = 10_000;
    cfg.max_account_b_settlement_chunks = 1;
    cfg.max_bankrupt_close_chunks = 1;
    cfg.max_bankrupt_close_lifetime_slots = 1;
    cfg.public_b_chunk_atoms = 1;
    percolator_prog::state::init_market_account_zero_copy(
        &mut data,
        &wrapper,
        cfg,
        slab.to_bytes(),
        initial_price,
        init_slot,
    )
    .expect("manual percolator market init");
    data
}

fn create_mint(svm: &mut LiteSVM, payer: &Keypair, authority: &Pubkey) -> Pubkey {
    let mint = Keypair::new();
    let rent = svm.minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN);
    let ixs = [
        system_instruction::create_account(
            &payer.pubkey(),
            &mint.pubkey(),
            rent,
            spl_token::state::Mint::LEN as u64,
            &spl_token::ID,
        ),
        spl_token::instruction::initialize_mint(&spl_token::ID, &mint.pubkey(), authority, None, 6)
            .unwrap(),
    ];
    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&payer.pubkey()),
        &[payer, &mint],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).unwrap();
    mint.pubkey()
}

fn token_account_data(mint: &Pubkey, owner: &Pubkey, amount: u64) -> Vec<u8> {
    let mut data = vec![0u8; spl_token::state::Account::LEN];
    let acc = spl_token::state::Account {
        mint: *mint,
        owner: *owner,
        amount,
        state: spl_token::state::AccountState::Initialized,
        ..Default::default()
    };
    spl_token::state::Account::pack(acc, &mut data).unwrap();
    data
}

fn create_token_account(svm: &mut LiteSVM, payer: &Keypair, mint: &Pubkey, owner: &Pubkey) -> Pubkey {
    let acc = Keypair::new();
    let rent = svm.minimum_balance_for_rent_exemption(spl_token::state::Account::LEN);
    let ixs = [
        system_instruction::create_account(
            &payer.pubkey(),
            &acc.pubkey(),
            rent,
            spl_token::state::Account::LEN as u64,
            &spl_token::ID,
        ),
        spl_token::instruction::initialize_account(&spl_token::ID, &acc.pubkey(), mint, owner)
            .unwrap(),
    ];
    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&payer.pubkey()),
        &[payer, &acc],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).unwrap();
    acc.pubkey()
}

fn mint_to(svm: &mut LiteSVM, payer: &Keypair, mint: &Pubkey, authority: &Keypair, dest: &Pubkey, amount: u64) {
    let ix =
        spl_token::instruction::mint_to(&spl_token::ID, mint, dest, &authority.pubkey(), &[], amount)
            .unwrap();
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&payer.pubkey()),
        &[payer, authority],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).unwrap();
}

/// Funds a depositor: airdrop SOL, create their ATA, mint `amount` to it.
fn new_depositor(env: &mut Env, amount: u64) -> (Keypair, Pubkey) {
    let kp = Keypair::new();
    env.svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
    let payer = clone_kp(&env.payer);
    let auth = clone_kp(&env.mint_auth);
    let mint = env.mint;
    let ata = create_token_account(&mut env.svm, &payer, &mint, &kp.pubkey());
    if amount > 0 {
        mint_to(&mut env.svm, &payer, &mint, &auth, &ata, amount);
    }
    (kp, ata)
}

// ---------------------------------------------------------------------------
// genesis-vote + distribution setup (for the vote-read step)
// ---------------------------------------------------------------------------

struct VoteEnv {
    gv_config: Pubkey,
    dist_config: Pubkey,
    coin_vault: Pubkey,
}

fn gv_config_pda(mint: &Pubkey, subledger_pool: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"gv_config", mint.as_ref(), subledger_pool.as_ref()], &gv_id()).0
}
fn dist_config_pda(mint: &Pubkey, authority: &Pubkey) -> Pubkey {
    // finding AA: the distribution config PDA binds its seal AUTHORITY (the gv config) into the
    // seed, so an attacker can't squat a funded config under a different authority.
    Pubkey::find_program_address(&[b"dist_config", mint.as_ref(), authority.as_ref()], &dist_id()).0
}

fn revoke_mint_authority(env: &mut Env, mint: &Pubkey) {
    let ix = spl_token::instruction::set_authority(
        &spl_token::ID,
        mint,
        None,
        spl_token::instruction::AuthorityType::MintTokens,
        &env.mint_auth.pubkey(),
        &[],
    )
    .unwrap();
    let auth = clone_kp(&env.mint_auth);
    env.send(&[ix], &[&auth]).expect("revoke mint authority");
}

fn setup_vote(env: &mut Env) -> VoteEnv {
    // gv + distribution are keyed by the COIN (a fixed-supply mint, distinct from
    // the collateral `env.mint` the subledger pool holds).
    let coin_mint = env.coin_mint;
    let gv_config = gv_config_pda(&coin_mint, &env.pool);
    let dist_config = dist_config_pda(&coin_mint, &gv_config);

    // distribution InitConfig with seal authority = the gv config PDA. Fund the COIN
    // vault, then REVOKE the COIN mint authority (the distribution requires a
    // fixed-supply COIN, README Safety §4).
    let dist_vault = create_token_account(&mut env.svm, &clone_kp(&env.payer), &coin_mint, &dist_config);
    mint_to(&mut env.svm, &clone_kp(&env.payer), &coin_mint, &clone_kp(&env.mint_auth), &dist_vault, 100);
    revoke_mint_authority(env, &coin_mint);
    let mut data = vec![0u8];
    data.extend_from_slice(&1_000_000u64.to_le_bytes()); // claim window
    data.extend_from_slice(&100u64.to_le_bytes()); // total supply
    let ix = Instruction {
        program_id: dist_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(coin_mint, false),
            AccountMeta::new(dist_config, false),
            AccountMeta::new_readonly(dist_vault, false),
            AccountMeta::new_readonly(gv_config, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data,
    };
    env.send(&[ix], &[]).expect("dist init");

    // genesis-vote InitConfig: stores the subledger program + pool to read at vote.
    let ix = Instruction {
        program_id: gv_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(coin_mint, false),
            AccountMeta::new(gv_config, false),
            AccountMeta::new_readonly(dist_id(), false),
            AccountMeta::new_readonly(dist_config, false),
            AccountMeta::new_readonly(sub_id(), false),   // subledger_program
            AccountMeta::new_readonly(env.pool, false),   // subledger_pool
            AccountMeta::new_readonly(Pubkey::default(), false), // reserved
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data: vec![0u8],
    };
    env.send(&[ix], &[]).expect("gv init");

    VoteEnv { gv_config, dist_config, coin_vault: dist_vault }
}

fn create_and_register_proposal(env: &mut Env, ve: &VoteEnv, id: u64, dest: &Pubkey) -> (Pubkey, Pubkey) {
    let dist_proposal =
        Pubkey::find_program_address(&[b"dist_proposal", ve.dist_config.as_ref(), &id.to_le_bytes()], &dist_id()).0;
    // create
    let mut data = vec![1u8];
    data.extend_from_slice(&id.to_le_bytes());
    data.extend_from_slice(&4u32.to_le_bytes());
    let create = Instruction {
        program_id: dist_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(ve.dist_config, false),
            AccountMeta::new(dist_proposal, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data,
    };
    env.send(&[create], &[]).expect("create proposal");
    // append one entry (full supply to `dest`).
    let mut ad = vec![2u8];
    ad.extend_from_slice(&1u32.to_le_bytes());
    ad.extend_from_slice(dest.as_ref());
    ad.extend_from_slice(&100u64.to_le_bytes());
    let append = Instruction {
        program_id: dist_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(ve.dist_config, false),
            AccountMeta::new(dist_proposal, false),
        ],
        data: ad,
    };
    env.send(&[append], &[]).expect("append");

    // genesis-vote register_proposal
    let gv_proposal =
        Pubkey::find_program_address(&[b"gv_proposal", ve.gv_config.as_ref(), dist_proposal.as_ref()], &gv_id()).0;
    let reg = Instruction {
        program_id: gv_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(ve.gv_config, false),
            AccountMeta::new(gv_proposal, false),
            AccountMeta::new_readonly(dist_proposal, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data: vec![2u8],
    };
    env.send(&[reg], &[]).expect("register");
    (dist_proposal, gv_proposal)
}

fn gv_vote(
    env: &mut Env,
    ve: &VoteEnv,
    voter: &Keypair,
    gv_proposal: &Pubkey,
    action: u8,
) -> Result<(), String> {
    let gv_ballot =
        Pubkey::find_program_address(&[b"gv_ballot", ve.gv_config.as_ref(), voter.pubkey().as_ref()], &gv_id()).0;
    let ix = Instruction {
        program_id: gv_id(),
        accounts: vec![
            AccountMeta::new(voter.pubkey(), true),
            AccountMeta::new(ve.gv_config, false),
            AccountMeta::new(gv_ballot, false),
            AccountMeta::new(*gv_proposal, false),
            AccountMeta::new(env.position_pda(&voter.pubkey()), false),
            AccountMeta::new_readonly(env.pool, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            AccountMeta::new_readonly(sub_id(), false),
        ],
        data: vec![3u8, action],
    };
    env.send(&[ix], &[voter])
}

// Permissionless winner-take-all trigger: seals the distribution to the winning
// proposal. One voter holding 100% trivially clears quorum + majority.
fn gv_trigger(env: &mut Env, ve: &VoteEnv, gv_proposal: &Pubkey, dist_proposal: &Pubkey) -> Result<(), String> {
    let ix = Instruction {
        program_id: gv_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new(ve.gv_config, false),
            AccountMeta::new(*gv_proposal, false),
            AccountMeta::new_readonly(dist_id(), false),
            AccountMeta::new(ve.dist_config, false),
            AccountMeta::new(*dist_proposal, false),
            AccountMeta::new_readonly(env.pool, false), // live quorum denominator
        ],
        data: vec![4u8],
    };
    env.send(&[ix], &[])
}

fn gv_proposal_support(env: &Env, gv_proposal: &Pubkey) -> (u64, u64) {
    let acc = env.svm.get_account(gv_proposal).unwrap();
    let support_weight = u64::from_le_bytes(acc.data[72..80].try_into().unwrap());
    let support_principal = u64::from_le_bytes(acc.data[80..88].try_into().unwrap());
    (support_weight, support_principal)
}

// ===========================================================================
// Tests
// ===========================================================================

#[test]
fn deposit_into_real_percolator_insurance_records_position() {
    let mut env = Env::new();
    env.init_insurance_pool();

    let amount = 1_000_000u64;
    let (alice, alice_ata) = new_depositor(&mut env, amount);
    let pool = env.pool;
    let holding = create_holding(&mut env, &pool);

    let before = env.token_amount(&env.perc_vault.clone());
    env.insurance_deposit(&alice, &alice_ata, &holding, amount).expect("insurance deposit");
    let after = env.token_amount(&env.perc_vault.clone());

    // Funds landed in the REAL Percolator insurance vault.
    assert_eq!(after - before, amount, "percolator insurance balance rose by deposit");
    assert_eq!(env.token_amount(&alice_ata), 0, "user ATA drained");

    // Position records principal + a nonzero start_slot; outstanding tracked.
    let (principal, start_slot, withdrawn) = env.read_position(&alice.pubkey());
    assert_eq!(principal, amount);
    assert_eq!(start_slot, 100, "start_slot = clock at deposit");
    assert!(!withdrawn);
    assert_eq!(env.pool_outstanding(), amount);
}

// Venue haircut behaviour of the insurance exit, against real percolator (finding L FIXED).
//
// SURPLUS: correctly EXCLUDED. percolator caps each WithdrawInsuranceLimited to
// `insurance*max_bps/1e4` then `min(deposit_remaining)`; with deposits_only=1 the cap
// is the deposited principal, so market profit/surplus is never withdrawable here.
//
// HAIRCUT: now PRO-RATA, not first-come. insurance_withdraw reads the LIVE asset-0 insurance
// straight from the slab; under an impairment (a venue loss that draws insurance below total
// outstanding principal) every exit receives insurance*amount/outstanding instead of the full
// principal. So a loss is shared proportionally and the exit is ORDER-INDEPENDENT — both an
// early and a late depositor take the SAME haircut; the first exit can no longer drain the
// pool and strand the rest.
#[test]
fn impaired_insurance_exit_is_pro_rata() {
    let mut env = Env::new();
    env.init_insurance_pool();

    let amount = 1_000_000u64;
    let (alice, alice_ata) = new_depositor(&mut env, amount);
    let (bob, bob_ata) = new_depositor(&mut env, amount);
    let pool = env.pool;
    let a_hold = create_holding(&mut env, &pool);
    let b_hold = create_holding(&mut env, &pool);
    env.insurance_deposit(&alice, &alice_ata, &a_hold, amount).expect("alice deposit");
    env.insurance_deposit(&bob, &bob_ata, &b_hold, amount).expect("bob deposit");
    assert_eq!(env.token_amount(&env.perc_vault.clone()), 2 * amount, "insurance funded by both");
    assert_eq!(env.pool_outstanding(), 2 * amount, "outstanding = both deposits");

    // Simulate a 50% venue loss: the market drew half the insurance to cover trader losses.
    // A real loss debits the insurance fund, the vault, the per-domain budgets and the
    // remaining-budget total together, so we mirror that exactly against the real slab layout
    // (otherwise percolator's `validate_shape` invariant insurance >= domain-budget-remaining
    // rejects the next withdraw with EngineLockActive). After this the authoritative `insurance`
    // figure is 1M < outstanding 2M -> impaired.
    impair_market(&mut env, amount as u128);
    // Vault token balance drops to the same 1M (the other 1M was paid out covering the loss).
    env.svm
        .set_account(
            env.perc_vault,
            Account {
                lamports: 1_000_000,
                data: token_account_data(&env.mint, &env.vault_authority, amount),
                owner: spl_token::ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();

    // Alice (early) exits her full principal but receives only her pro-rata share:
    // insurance(1,000,000) * amount(1,000,000) / outstanding(2,000,000) = 500,000 (a 50% haircut).
    env.insurance_withdraw(&alice, &alice_ata, &a_hold, &alice, amount).expect("alice exits");
    assert_eq!(env.token_amount(&alice_ata), 500_000, "early depositor takes the 50% haircut, not the full principal");
    assert_eq!(env.token_amount(&env.perc_vault.clone()), 500_000, "half the impaired insurance remains for bob");
    assert_eq!(env.pool_outstanding(), amount, "alice's full principal left the outstanding accounting");

    // Bob (late) gets the SAME 50% haircut — the pool was NOT drained by the first exit.
    // insurance now 500,000, outstanding now 1,000,000 -> 500,000 * 1,000,000 / 1,000,000 = 500,000.
    env.insurance_withdraw(&bob, &bob_ata, &b_hold, &bob, amount).expect("bob exits with the same haircut");
    assert_eq!(env.token_amount(&bob_ata), 500_000, "late depositor takes the SAME 50% haircut — order-independent");
    assert_eq!(env.token_amount(&env.perc_vault.clone()), 0, "impaired insurance fully and fairly distributed");
}

// LAMPORT PRE-FUND INIT-DOS (finding AI): every init handler creates its PDA with the System
// `create_account`, which FAILS with AccountAlreadyInUse if the destination already holds ANY
// lamports — and the handlers additionally guard `lamports() != 0 -> AlreadyInitialized`. An attacker
// can transfer 1 lamport to the deterministic pool PDA (a transfer needs NO destination signature)
// BEFORE the genesis init, permanently bricking init_insurance_pool — and with it the whole genesis,
// since the lamports can never be swept (no one can sign for a system-owned PDA, and the legit init
// keeps rejecting). The robust create (top-up the rent shortfall, then allocate + assign via
// invoke_signed) tolerates the pre-funding because allocate/assign only require data-empty +
// system-owned, not zero lamports. This test dusts the PDA and asserts init STILL succeeds.
#[test]
fn lamport_prefund_cannot_brick_insurance_pool_init() {
    let mut env = Env::new();
    env.svm.set_account(env.pool, Account {
        lamports: 1, // attacker dust
        data: vec![],
        owner: solana_sdk::system_program::ID,
        executable: false,
        rent_epoch: 0,
    }).unwrap();
    env.init_insurance_pool(); // must still succeed (robust create handles the pre-funded PDA)
    let acc = env.svm.get_account(&env.pool).unwrap();
    assert_eq!(acc.owner, sub_id(), "pool created + owned by subledger despite the dust");
    assert!(acc.data.len() >= 88, "pool data initialized");
}

// RE-INIT PROTECTION (regression guard for finding AI): the finding-AI fix relaxed the init guard
// from `lamports() != 0 || data_len() != 0` to `data_len() != 0` so a dusted-but-empty PDA can still
// be created. This must NOT weaken re-init protection — an already-initialized pool has data, so a
// second init_insurance_pool on the same PDA must still be rejected. Otherwise an attacker could
// re-init a LIVE pool and reset pool.outstanding_principal (the genesis quorum denominator) to 0, or
// re-point its vault/policy — a state-reset governance/LOF attack. Previously untested stack-wide.
#[test]
fn insurance_pool_cannot_be_reinitialized_after_funding() {
    let mut env = Env::new();
    env.init_insurance_pool();
    let pool = env.pool;
    let (alice, alice_ata) = new_depositor(&mut env, 1_000_000);
    let hold = create_holding(&mut env, &pool);
    env.insurance_deposit(&alice, &alice_ata, &hold, 1_000_000).expect("deposit");
    assert_eq!(env.pool_outstanding(), 1_000_000, "pool has live outstanding");

    // ATTACK: re-init the SAME pool PDA (would zero outstanding / re-point bindings if it succeeded).
    let mut data = vec![3u8]; // IX_INIT_INSURANCE_POOL
    data.extend_from_slice(&ASSET_ID.to_le_bytes());
    data.push(POLICY_PRINCIPAL);
    let reinit = Instruction {
        program_id: sub_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(env.mint, false),
            AccountMeta::new(pool, false),
            AccountMeta::new_readonly(env.perc_vault, false),
            AccountMeta::new_readonly(env.slab, false),
            AccountMeta::new_readonly(perc_id(), false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            AccountMeta::new_readonly(gv_config_pda(&env.coin_mint, &pool), false),
        ],
        data,
    };
    assert!(env.send(&[reinit], &[]).is_err(), "re-init of a live pool must be rejected (data_len guard)");
    assert_eq!(env.pool_outstanding(), 1_000_000, "outstanding (quorum denominator) untouched by the blocked re-init");
}

// DEPOSIT MARKET-BINDING (Sybil-resistance core): vote weight must be backed by capital genuinely at
// risk in the GENESIS market. insurance_deposit credits position.principal (which becomes vote weight)
// and CPIs TopUpInsurance to move the capital into the pool's bound market. If a depositor could pass
// a FOREIGN market_slab (one they control) while depositing to the genesis pool, they'd get a credited
// position while routing capital somewhere they can reclaim it — free governance power, defeating the
// whole Sybil check. deposit pins market_slab == pool.market_slab (+ vault + program). Distinct code
// path from the withdraw foreign-slab pin (finding AF): a regression dropping the deposit pin would
// NOT be caught by AF. Previously untested.
#[test]
fn deposit_with_foreign_market_slab_credits_no_position() {
    let mut env = Env::new();
    env.init_insurance_pool();
    let pool = env.pool;
    let (attacker, atk_ata) = new_depositor(&mut env, 1_000_000);
    let hold = create_holding(&mut env, &pool);

    // A DIFFERENT live market the attacker would rather route capital to (clone at a fresh address).
    let foreign_slab = Pubkey::new_unique();
    let fs = env.svm.get_account(&env.slab).unwrap();
    env.svm.set_account(foreign_slab, fs).unwrap();

    // ATTACK: deposit to the genesis pool but point market_slab at the foreign market.
    let mut data = vec![4u8];
    data.extend_from_slice(&1_000_000u64.to_le_bytes());
    let attack = Instruction {
        program_id: sub_id(),
        accounts: vec![
            AccountMeta::new(attacker.pubkey(), true),
            AccountMeta::new(pool, false),
            AccountMeta::new(env.position_pda(&attacker.pubkey()), false),
            AccountMeta::new(atk_ata, false),
            AccountMeta::new(hold, false),
            AccountMeta::new(foreign_slab, false), // <-- substituted market
            AccountMeta::new(env.perc_vault, false),
            AccountMeta::new_readonly(perc_id(), false),
            AccountMeta::new_readonly(spl_token::ID, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data,
    };
    assert!(env.send(&[attack], &[&attacker]).is_err(), "deposit with a foreign market_slab must be rejected");
    let pos = env.svm.get_account(&env.position_pda(&attacker.pubkey()));
    assert!(pos.is_none() || pos.unwrap().data.is_empty(), "no credited position from the blocked deposit");
    assert_eq!(env.pool_outstanding(), 0, "no free vote weight credited");
    assert_eq!(env.token_amount(&atk_ata), 1_000_000, "attacker's capital untouched");
}

// CROSS-MARKET HAIRCUT-BASIS SUBSTITUTION (LOF): the pro-rata exit reads the live insurance basis
// from the passed market_slab (findings L + T). If a depositor in an IMPAIRED pool could pass a
// DIFFERENT, HEALTHY market's slab, payout() would read that market's full insurance and treat the
// exit as un-impaired — paying FULL principal while the pull still drains the real (impaired) market,
// stealing the loss-share owed to the remaining depositors. Defense: withdraw pins
// market_slab == pool.market_slab (subledger/src/lib.rs) BEFORE it reads insurance or signs the
// WithdrawInsuranceLimited pull. Symmetric to the twap's
// e2e_execute_rejects_foreign_market_vault_authority; previously untested on the subledger side.
#[test]
fn foreign_market_slab_cannot_inflate_the_haircut() {
    let mut env = Env::new();
    env.init_insurance_pool();
    let amount = 1_000_000u64;
    let (alice, alice_ata) = new_depositor(&mut env, amount);
    let pool = env.pool;
    let a_hold = create_holding(&mut env, &pool);
    env.insurance_deposit(&alice, &alice_ata, &a_hold, amount).expect("alice deposit");

    // A DIFFERENT, HEALTHY market (2M insurance) — the bait the attacker wants payout() to read.
    let foreign_slab = Pubkey::new_unique();
    let mut fs = env.svm.get_account(&env.slab).unwrap();
    let off_ins = MARKET_GROUP_OFF + 301;
    fs.data[off_ins..off_ins + 16].copy_from_slice(&2_000_000u128.to_le_bytes());
    env.svm.set_account(foreign_slab, fs).unwrap();

    // Impair the REAL market to 50%: an honest exit owes only 500k (insurance 500k / outstanding 1M).
    impair_market(&mut env, 500_000u128);
    env.svm.set_account(env.perc_vault, Account {
        lamports: 1_000_000,
        data: token_account_data(&env.mint, &env.vault_authority, 500_000),
        owner: spl_token::ID, executable: false, rent_epoch: 0,
    }).unwrap();

    // ATTACK: withdraw with market_slab pointing at the HEALTHY foreign market to read its 2M basis.
    let mut d = vec![5u8]; d.extend_from_slice(&amount.to_le_bytes());
    let attack = Instruction {
        program_id: sub_id(),
        accounts: vec![
            AccountMeta::new(alice.pubkey(), true),
            AccountMeta::new(pool, false),
            AccountMeta::new(env.position_pda(&alice.pubkey()), false),
            AccountMeta::new(alice_ata, false),
            AccountMeta::new(a_hold, false),
            AccountMeta::new(foreign_slab, false), // <-- substituted slab
            AccountMeta::new(env.perc_vault, false),
            AccountMeta::new_readonly(env.vault_authority, false),
            AccountMeta::new_readonly(perc_id(), false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data: d,
    };
    assert!(env.send(&[attack], &[&alice]).is_err(), "a foreign market_slab must be rejected (key != pool.market_slab)");
    let (_, _, withdrawn) = env.read_position(&alice.pubkey());
    assert!(!withdrawn, "the position is untouched after the blocked attack");
    assert_eq!(env.token_amount(&alice_ata), 0, "no funds extracted via the foreign slab");

    // The honest exit (real slab) pays only the 50% haircut — the foreign slab bought no advantage.
    env.insurance_withdraw(&alice, &alice_ata, &a_hold, &alice, amount).expect("honest exit");
    assert_eq!(env.token_amount(&alice_ata), 500_000, "honest pro-rata haircut is 500k, never the 1M a healthy basis would pay");
}

// Full genesis lifecycle with ALL real programs (percolator + subledger +
// genesis-vote + distribution): a depositor puts collateral at risk in percolator
// insurance, votes, the permissionless trigger seals the winning distribution by CPI,
// and the winning recipient CLAIMS the fixed-supply COIN. Pins that the whole chain
// produces a claimable distribution end-to-end (a broken link here bricks the genesis).
#[test]
fn full_lifecycle_deposit_vote_seal_then_recipient_claims_coin() {
    let mut env = Env::new();
    env.init_insurance_pool();
    let ve = setup_vote(&mut env);

    // The depositor (voter) and a separate COIN recipient named by the proposal.
    let amount = 1_000_000u64;
    let (alice, alice_ata) = new_depositor(&mut env, amount);
    let pool = env.pool;
    let holding = create_holding(&mut env, &pool);
    env.insurance_deposit(&alice, &alice_ata, &holding, amount).expect("collateral deposit");

    let recipient = Keypair::new();
    let recipient_coin_ata =
        create_token_account(&mut env.svm, &clone_kp(&env.payer), &env.coin_mint, &recipient.pubkey());

    // Proposal allocates the full COIN supply (100) to the recipient.
    let (dist_proposal, gv_proposal) = create_and_register_proposal(&mut env, &ve, 1, &recipient.pubkey());

    // Vote it to quorum + majority, then permissionlessly trigger the seal.
    env.warp_slot(1124);
    gv_vote(&mut env, &ve, &alice, &gv_proposal, 1).expect("vote");
    gv_trigger(&mut env, &ve, &gv_proposal, &dist_proposal).expect("trigger seals the distribution");

    // The recipient claims their COIN from the sealed distribution.
    assert_eq!(env.token_amount(&recipient_coin_ata), 0, "nothing before claim");
    let mut data = vec![4u8]; // IX_CLAIM
    data.extend_from_slice(&0u32.to_le_bytes()); // index 0
    let claim = Instruction {
        program_id: dist_id(),
        accounts: vec![
            AccountMeta::new_readonly(recipient.pubkey(), true),
            AccountMeta::new_readonly(ve.dist_config, false),
            AccountMeta::new(dist_proposal, false),
            AccountMeta::new(ve.coin_vault, false),
            AccountMeta::new(recipient_coin_ata, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data,
    };
    env.send(&[claim], &[&recipient]).expect("recipient claims the COIN");
    assert_eq!(env.token_amount(&recipient_coin_ata), 100, "winner received the full COIN pool");

    // Re-claiming the same entry is refused (entry zeroed).
    let mut data = vec![4u8];
    data.extend_from_slice(&0u32.to_le_bytes());
    let reclaim = Instruction {
        program_id: dist_id(),
        accounts: vec![
            AccountMeta::new_readonly(recipient.pubkey(), true),
            AccountMeta::new_readonly(ve.dist_config, false),
            AccountMeta::new(dist_proposal, false),
            AccountMeta::new(ve.coin_vault, false),
            AccountMeta::new(recipient_coin_ata, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data,
    };
    assert!(env.send(&[reclaim], &[&recipient]).is_err(), "cannot double-claim");
}

// Anti bait-and-switch: a creator must not be able to change the distribution after
// voters have backed it. Build a PARTIAL proposal (room to append), register + vote
// it, then append a self-allocation — the trigger must REFUSE to seal the changed
// proposal (its entry_count/total_amount snapshot no longer matches).
#[test]
fn proposal_changed_after_registration_cannot_be_sealed() {
    let mut env = Env::new();
    env.init_insurance_pool();
    let ve = setup_vote(&mut env);
    let dist_config = ve.dist_config;

    let amount = 1_000_000u64;
    let (alice, alice_ata) = new_depositor(&mut env, amount);
    let pool = env.pool;
    let holding = create_holding(&mut env, &pool);
    env.insurance_deposit(&alice, &alice_ata, &holding, amount).expect("deposit");

    let id = 1u64;
    let dist_proposal =
        Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &id.to_le_bytes()], &dist_id()).0;
    let mut cd = vec![1u8];
    cd.extend_from_slice(&id.to_le_bytes());
    cd.extend_from_slice(&4u32.to_le_bytes());
    env.send(&[Instruction {
        program_id: dist_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(dist_config, false),
            AccountMeta::new(dist_proposal, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data: cd,
    }], &[]).expect("create proposal");

    let append = |env: &mut Env, dest: &Pubkey, amt: u64| -> Result<(), String> {
        let mut ad = vec![2u8];
        ad.extend_from_slice(&1u32.to_le_bytes());
        ad.extend_from_slice(dest.as_ref());
        ad.extend_from_slice(&amt.to_le_bytes());
        env.send(&[Instruction {
            program_id: dist_id(),
            accounts: vec![
                AccountMeta::new(env.payer.pubkey(), true),
                AccountMeta::new_readonly(dist_config, false),
                AccountMeta::new(dist_proposal, false),
            ],
            data: ad,
        }], &[])
    };
    // A fair partial allocation (40 of 100): leaves room to append later.
    let fair = Pubkey::new_unique();
    append(&mut env, &fair, 40).expect("append fair entry");

    // Register the gv proposal — snapshots (entry_count=1, total_amount=40).
    let gv_proposal =
        Pubkey::find_program_address(&[b"gv_proposal", ve.gv_config.as_ref(), dist_proposal.as_ref()], &gv_id()).0;
    env.send(&[Instruction {
        program_id: gv_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(ve.gv_config, false),
            AccountMeta::new(gv_proposal, false),
            AccountMeta::new_readonly(dist_proposal, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data: vec![2u8],
    }], &[]).expect("register");

    // Voters back it to quorum + majority.
    env.warp_slot(1124);
    gv_vote(&mut env, &ve, &alice, &gv_proposal, 1).expect("vote");

    // ATTACK: the creator appends a self-allocation AFTER voters committed.
    let attacker = Pubkey::new_unique();
    append(&mut env, &attacker, 60).expect("creator can still append (no dist-level lock)");

    // The trigger must refuse to seal the changed proposal.
    assert!(
        gv_trigger(&mut env, &ve, &gv_proposal, &dist_proposal).is_err(),
        "trigger must reject a proposal changed after registration"
    );
}

#[test]
fn genesis_vote_reads_subledger_position_and_weights() {
    let mut env = Env::new();
    env.init_insurance_pool();
    let ve = setup_vote(&mut env);

    let amount = 1_000_000u64;
    let (alice, alice_ata) = new_depositor(&mut env, amount);
    let pool = env.pool;
    let holding = create_holding(&mut env, &pool);
    env.insurance_deposit(&alice, &alice_ata, &holding, amount).expect("deposit");
    // deposit at slot 100.

    let dest = Pubkey::new_unique();
    let (_dist_proposal, gv_proposal) = create_and_register_proposal(&mut env, &ve, 1, &dest);

    // Advance the clock so hold = 1124 - 100 = 1024 -> floor(log2(1024)) = 10.
    env.warp_slot(1124);
    gv_vote(&mut env, &ve, &alice, &gv_proposal, 1).expect("vote backs proposal");

    let (support_weight, support_principal) = gv_proposal_support(&env, &gv_proposal);
    assert_eq!(support_principal, amount);
    // weight = floor(log2(hold)) * principal = 10 * 1_000_000.
    assert_eq!(support_weight, 10 * amount, "weight = floor(log2(hold)) * principal");
}

// Finding B (vote-outlives-capital): a live genesis ballot must keep its principal
// at risk. Before the fix, a voter could vote (recording a principal/weight snapshot)
// then insurance-withdraw their capital, leaving a free, capital-less ballot that
// still counted toward quorum/majority — worse after the live-outstanding fix, since
// withdrawing shrinks the denominator while the snapshot numerator stays. Now the
// genesis-vote CPIs the subledger to lock the position while the ballot is live;
// withdraw is refused until the voter retracts (which clears the lock).
#[test]
fn vote_locked_principal_cannot_exit_until_retracted() {
    let mut env = Env::new();
    env.init_insurance_pool();
    let ve = setup_vote(&mut env);

    let amount = 1_000_000u64;
    let (alice, alice_ata) = new_depositor(&mut env, amount);
    let pool = env.pool;
    let holding = create_holding(&mut env, &pool);
    env.insurance_deposit(&alice, &alice_ata, &holding, amount).expect("deposit");

    let dest = Pubkey::new_unique();
    let (_dist_proposal, gv_proposal) = create_and_register_proposal(&mut env, &ve, 1, &dest);

    let vote_locked = |env: &Env| -> bool {
        env.svm.get_account(&env.position_pda(&alice.pubkey())).unwrap().data[97] == 1
    };

    // Before voting: not locked, and a withdraw would be allowed.
    assert!(!vote_locked(&env), "fresh position is not vote-locked");

    // Vote → the genesis-vote CPI locks the position.
    env.warp_slot(1124);
    gv_vote(&mut env, &ve, &alice, &gv_proposal, 1).expect("vote backs proposal");
    assert!(vote_locked(&env), "voting locks the principal");

    // The attack: try to withdraw the capital while the ballot is still live.
    let err = env.insurance_withdraw(&alice, &alice_ata, &holding, &alice, amount);
    assert!(err.is_err(), "vote-locked principal cannot be withdrawn");
    // Funds stayed in insurance; the position is intact.
    assert_eq!(env.token_amount(&env.perc_vault.clone()), amount, "capital still at risk");
    let (principal, _s, withdrawn) = env.read_position(&alice.pubkey());
    assert_eq!(principal, amount);
    assert!(!withdrawn);

    // Retract → the CPI clears the lock; the ballot's principal/weight is removed.
    gv_vote(&mut env, &ve, &alice, &gv_proposal, 2).expect("retract");
    assert!(!vote_locked(&env), "retract clears the lock");
    let (support_weight, support_principal) = gv_proposal_support(&env, &gv_proposal);
    assert_eq!(support_weight, 0, "retract removes the ballot's weight");
    assert_eq!(support_principal, 0, "retract removes the ballot's principal");

    // Now the exit succeeds: capital can only leave once it no longer backs a vote.
    env.insurance_withdraw(&alice, &alice_ata, &holding, &alice, amount).expect("exit after retract");
    assert_eq!(env.token_amount(&alice_ata), amount, "principal returned post-retract");
    assert_eq!(env.token_amount(&env.perc_vault.clone()), 0, "insurance drained");
}

// Cross-config binding (finalize-DOS): a vote may only be registered against a
// distribution proposal that belongs to THIS genesis's distribution config. A
// proposal owned by the distribution program but under a DIFFERENT config, if it
// won, could never be sealed (trigger CPIs SealWinner with config.distribution_config,
// which the distribution rejects on header.config mismatch) — bricking finalize
// forever. register_proposal must refuse to bind such a proposal up front.
#[test]
fn register_rejects_foreign_distribution_proposal() {
    let mut env = Env::new();
    env.init_insurance_pool();
    let ve = setup_vote(&mut env); // genesis distribution config is under env.mint

    // Build a FOREIGN, fully-legitimate distribution config under a different mint.
    let foreign_mint = create_mint(&mut env.svm, &clone_kp(&env.payer), &env.mint_auth.pubkey());
    let foreign_authority = Pubkey::new_unique();
    let foreign_config = dist_config_pda(&foreign_mint, &foreign_authority);
    let foreign_vault = create_token_account(&mut env.svm, &clone_kp(&env.payer), &foreign_mint, &foreign_config);
    mint_to(&mut env.svm, &clone_kp(&env.payer), &foreign_mint, &clone_kp(&env.mint_auth), &foreign_vault, 100);
    revoke_mint_authority(&mut env, &foreign_mint); // fixed-supply COIN (Safety §4)
    let mut data = vec![0u8]; // IX_INIT_CONFIG
    data.extend_from_slice(&1_000_000u64.to_le_bytes());
    data.extend_from_slice(&100u64.to_le_bytes());
    let init = Instruction {
        program_id: dist_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(foreign_mint, false),
            AccountMeta::new(foreign_config, false),
            AccountMeta::new_readonly(foreign_vault, false),
            AccountMeta::new_readonly(foreign_authority, false), // bound into the config seed (finding AA)
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data,
    };
    env.send(&[init], &[]).expect("foreign dist config init");

    // A proposal + entry under the FOREIGN config.
    let id = 7u64;
    let foreign_proposal =
        Pubkey::find_program_address(&[b"dist_proposal", foreign_config.as_ref(), &id.to_le_bytes()], &dist_id()).0;
    let mut cd = vec![1u8];
    cd.extend_from_slice(&id.to_le_bytes());
    cd.extend_from_slice(&4u32.to_le_bytes());
    env.send(&[Instruction {
        program_id: dist_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(foreign_config, false),
            AccountMeta::new(foreign_proposal, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data: cd,
    }], &[]).expect("create foreign proposal");

    // Now try to register a genesis vote against that foreign proposal.
    let gv_proposal =
        Pubkey::find_program_address(&[b"gv_proposal", ve.gv_config.as_ref(), foreign_proposal.as_ref()], &gv_id()).0;
    let reg = Instruction {
        program_id: gv_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(ve.gv_config, false),
            AccountMeta::new(gv_proposal, false),
            AccountMeta::new_readonly(foreign_proposal, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data: vec![2u8],
    };
    let res = env.send(&[reg], &[]);
    assert!(res.is_err(), "must not register a vote against a foreign-config proposal");
    // The gv_proposal account was never created.
    assert!(env.svm.get_account(&gv_proposal).map_or(true, |a| a.data.is_empty()));

    // Sanity: a proposal under the genesis's OWN config still registers fine.
    let dest = Pubkey::new_unique();
    let (_dp, gv_ok) = create_and_register_proposal(&mut env, &ve, 1, &dest);
    assert!(env.svm.get_account(&gv_ok).is_some_and(|a| !a.data.is_empty()), "own-config proposal registers");
}

// The vote-lock must not become a permanent freeze. After the winner is sealed
// (pv.executed), a WINNING voter's position is still locked — they must be able to
// RETRACT post-seal to release the lock and exit their principal. (The seal is
// immutable; only NEW backing is forbidden post-seal.) Without this, the very
// voters who carried the winning proposal would have their capital frozen forever.
#[test]
fn winning_voter_can_retract_and_exit_after_finalize() {
    let mut env = Env::new();
    env.init_insurance_pool();
    let ve = setup_vote(&mut env);

    let amount = 1_000_000u64;
    let (alice, alice_ata) = new_depositor(&mut env, amount);
    let pool = env.pool;
    let holding = create_holding(&mut env, &pool);
    env.insurance_deposit(&alice, &alice_ata, &holding, amount).expect("deposit");

    let dest = Pubkey::new_unique();
    let (dist_proposal, gv_proposal) = create_and_register_proposal(&mut env, &ve, 1, &dest);

    env.warp_slot(1124);
    gv_vote(&mut env, &ve, &alice, &gv_proposal, 1).expect("vote");

    // Finalize: the single voter holds 100%, so quorum + majority both hold.
    gv_trigger(&mut env, &ve, &gv_proposal, &dist_proposal).expect("trigger seals the winner");

    // Still locked immediately post-seal: capital can't sneak out without retracting.
    let err = env.insurance_withdraw(&alice, &alice_ata, &holding, &alice, amount);
    assert!(err.is_err(), "still vote-locked post-seal until retracted");

    // The freeze fix: a winning voter can retract AFTER finalize (only new backing
    // is forbidden once sealed), which clears the subledger lock.
    gv_vote(&mut env, &ve, &alice, &gv_proposal, 2).expect("retract must be allowed post-seal");

    // ...and then recover their principal. No permanent freeze.
    env.insurance_withdraw(&alice, &alice_ata, &holding, &alice, amount).expect("exit after finalize+retract");
    assert_eq!(env.token_amount(&alice_ata), amount, "principal recovered after finalize");
}

// Griefing-freeze: init_insurance_pool is permissionless and records vote_authority
// as-is, so an attacker could front-run pool creation with a hostile vote_authority.
// That must NOT let them freeze depositors: set_vote_lock requires the position
// OWNER to sign, so a position can only be (un)locked when its owner is acting on
// their own vote. Here a hostile authority tries to lock a victim and fails; the
// victim's funds stay withdrawable.
#[test]
fn hostile_vote_authority_cannot_freeze_a_depositor() {
    let mut env = Env::new();
    let attacker = Keypair::new();
    env.svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();

    // Pool created with the ATTACKER as vote_authority (the front-run scenario).
    let mut data = vec![3u8]; // IX_INIT_INSURANCE_POOL
    data.extend_from_slice(&ASSET_ID.to_le_bytes());
    data.push(POLICY_PRINCIPAL);
    let init = Instruction {
        program_id: sub_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(env.mint, false),
            AccountMeta::new(env.pool, false),
            AccountMeta::new_readonly(env.perc_vault, false),
            AccountMeta::new_readonly(env.slab, false),
            AccountMeta::new_readonly(perc_id(), false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            AccountMeta::new_readonly(attacker.pubkey(), false), // hostile vote_authority
        ],
        data,
    };
    env.send(&[init], &[]).expect("init pool with hostile authority");

    let amount = 1_000_000u64;
    let (victim, victim_ata) = new_depositor(&mut env, amount);
    let pool = env.pool;
    let holding = create_holding(&mut env, &pool);
    env.insurance_deposit(&victim, &victim_ata, &holding, amount).expect("deposit");

    // Attacker signs as the vote_authority and tries to lock the victim's position
    // WITHOUT the victim's signature (victim passed as a non-signer account).
    let attack = Instruction {
        program_id: sub_id(),
        accounts: vec![
            AccountMeta::new_readonly(attacker.pubkey(), true),
            AccountMeta::new_readonly(env.pool, false),
            AccountMeta::new(env.position_pda(&victim.pubkey()), false),
            AccountMeta::new_readonly(victim.pubkey(), false), // owner NOT signing
        ],
        data: vec![6u8, 1u8], // IX_SET_VOTE_LOCK, locked=1
    };
    let res = env.send(&[attack], &[&attacker]);
    assert!(res.is_err(), "cannot lock a position the owner did not sign for");

    // The victim is not frozen — their principal is still withdrawable.
    env.insurance_withdraw(&victim, &victim_ata, &holding, &victim, amount).expect("victim can still exit");
    assert_eq!(env.token_amount(&victim_ata), amount, "depositor funds were never frozen");
}

#[test]
fn principal_only_owner_exit_returns_funds_and_guards() {
    let mut env = Env::new();
    env.init_insurance_pool();

    let amount = 1_000_000u64;
    let (alice, alice_ata) = new_depositor(&mut env, amount);
    let pool = env.pool;
    let holding = create_holding(&mut env, &pool);
    env.insurance_deposit(&alice, &alice_ata, &holding, amount).expect("deposit");
    assert_eq!(env.token_amount(&env.perc_vault.clone()), amount);

    // A non-owner cannot withdraw the owner's position.
    let (mallory, _mallory_ata) = new_depositor(&mut env, 0);
    let err = env.insurance_withdraw(&alice, &alice_ata, &holding, &mallory, 1);
    assert!(err.is_err(), "non-owner cannot withdraw");

    // Cannot withdraw more than the recorded principal.
    let err = env.insurance_withdraw(&alice, &alice_ata, &holding, &alice, amount + 1);
    assert!(err.is_err(), "cannot exceed recorded principal");

    // Partial principal-only exit.
    env.insurance_withdraw(&alice, &alice_ata, &holding, &alice, 400_000).expect("partial exit");
    assert_eq!(env.token_amount(&alice_ata), 400_000, "user got partial principal back");
    assert_eq!(env.token_amount(&env.perc_vault.clone()), 600_000, "insurance decreased");
    let (principal, _start, withdrawn) = env.read_position(&alice.pubkey());
    assert_eq!(principal, 600_000);
    assert!(!withdrawn);
    assert_eq!(env.pool_outstanding(), 600_000);

    // Exit the remainder.
    env.insurance_withdraw(&alice, &alice_ata, &holding, &alice, 600_000).expect("full exit");
    assert_eq!(env.token_amount(&alice_ata), amount, "user got all principal back");
    assert_eq!(env.token_amount(&env.perc_vault.clone()), 0, "insurance drained");
    let (principal, _start, withdrawn) = env.read_position(&alice.pubkey());
    assert_eq!(principal, 0);
    assert!(withdrawn, "position retired at zero principal");
    assert_eq!(env.pool_outstanding(), 0);

    // A retired position cannot be withdrawn again.
    let err = env.insurance_withdraw(&alice, &alice_ata, &holding, &alice, 1);
    assert!(err.is_err(), "retired position cannot withdraw");
}

// Type-confusion boundary: the own-vault deposit path (tag 1) must REJECT an
// insurance pool. An insurance pool's `vault` is the percolator insurance vault,
// owned by the percolator vault_authority — not this pool PDA. Without the guard,
// an own-vault deposit would SPL-transfer the user's funds straight into that
// vault with NO TopUpInsurance CPI (percolator never counts them) and record an
// own-vault position; the matching own-vault withdraw could never sign those
// funds back out (the pool PDA is not the vault's token authority) → the user's
// principal is stranded. This pins that the misuse is refused up front.
#[test]
fn own_vault_deposit_is_rejected_on_an_insurance_pool() {
    let mut env = Env::new();
    env.init_insurance_pool();

    let amount = 1_000_000u64;
    let (alice, alice_ata) = new_depositor(&mut env, amount);
    let pool = env.pool;

    // Own-vault deposit (IX_DEPOSIT = 1) aimed at the insurance pool, with the
    // insurance vault passed as the own-vault `vault`. The guard must fire before
    // any token movement.
    let mut data = vec![1u8];
    data.extend_from_slice(&amount.to_le_bytes());
    let ix = Instruction {
        program_id: sub_id(),
        accounts: vec![
            AccountMeta::new(alice.pubkey(), true),
            AccountMeta::new(pool, false),
            AccountMeta::new(env.position_pda(&alice.pubkey()), false),
            AccountMeta::new(alice_ata, false),
            AccountMeta::new(env.perc_vault, false),
            AccountMeta::new_readonly(spl_token::ID, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data,
    };
    let res = env.send(&[ix], &[&alice]);
    assert!(res.is_err(), "own-vault deposit must be refused on an insurance pool");

    // And the user's funds never moved into the insurance vault.
    assert_eq!(env.token_amount(&alice_ata), amount, "depositor funds untouched");
    assert_eq!(env.token_amount(&env.perc_vault.clone()), 0, "insurance vault untouched");
}

// Canonical-vault pin (issue #24, active path): init_insurance_pool must reject a
// vault that is owned by the correct vault_authority and holds the correct mint
// but is NOT the canonical ATA. Percolator (F-VAULT-FRAG) would reject such a
// vault on every deposit/withdraw CPI, so binding a pool to it leaves the pool
// permanently inert. Pinning the canonical address at init fails fast instead.
#[test]
fn init_insurance_pool_rejects_non_canonical_vault() {
    let mut env = Env::new();

    // A second token account owned by the very same vault_authority, correct mint,
    // but at a fresh (non-canonical) address.
    let rogue_vault = Pubkey::new_unique();
    env.svm
        .set_account(
            rogue_vault,
            solana_sdk::account::Account {
                lamports: 1_000_000_000,
                data: token_account_data(&env.mint, &env.vault_authority, 0),
                owner: spl_token::ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    assert_ne!(rogue_vault, env.perc_vault, "precondition: not the canonical ATA");

    let mut data = vec![3u8]; // IX_INIT_INSURANCE_POOL
    data.extend_from_slice(&ASSET_ID.to_le_bytes());
    data.push(POLICY_PRINCIPAL);
    let ix = Instruction {
        program_id: sub_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(env.mint, false),
            AccountMeta::new(env.pool, false),
            AccountMeta::new_readonly(rogue_vault, false),
            AccountMeta::new_readonly(env.slab, false),
            AccountMeta::new_readonly(perc_id(), false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            AccountMeta::new_readonly(gv_config_pda(&env.coin_mint, &env.pool), false),
        ],
        data,
    };
    let res = env.send(&[ix], &[]);
    assert!(res.is_err(), "init must reject a non-canonical vault");

    // The pool account was never created, so the canonical path still works.
    assert!(env.svm.get_account(&env.pool).map_or(true, |a| a.data.is_empty()));
    env.init_insurance_pool();
}

// Verify the percolator UpdateAssetAuthority encoding the TWAP handoff bridge
// (twap-program IX_ACCEPT_OPERATOR) relies on — tag 65, asset_index 0,
// kind=INSURANCE_OPERATOR(2), accounts [current(signer), new(signer), market(w)] —
// against the REAL percolator binary, so the handoff can't silently fail.
#[test]
fn percolator_update_asset_authority_operator_encoding_is_accepted() {
    let mut svm = LiteSVM::new().with_compute_budget(ComputeBudget {
        compute_unit_limit: 1_400_000,
        heap_size: 256 * 1024,
        ..ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let mint_auth = Keypair::new();
    let mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());

    let admin = Keypair::new(); // marketauth -> asset-0 asset_admin
    let slab = Pubkey::new_unique();
    let init_slot = 100u64;
    let slab_data = make_live_market(&slab, &mint, &admin.pubkey(), init_slot);
    svm.set_account(
        slab,
        Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 },
    )
    .unwrap();
    svm.set_sysvar(&Clock { slot: init_slot, unix_timestamp: 100, ..Clock::default() });

    let new_op = Keypair::new();
    let mut data = vec![65u8]; // IX_UPDATE_ASSET_AUTHORITY
    data.extend_from_slice(&0u16.to_le_bytes()); // asset_index 0
    data.push(2u8); // ASSET_AUTH_INSURANCE_OPERATOR
    data.extend_from_slice(new_op.pubkey().as_ref());
    let ix = Instruction {
        program_id: perc_id(),
        accounts: vec![
            AccountMeta::new_readonly(admin.pubkey(), true),
            AccountMeta::new_readonly(new_op.pubkey(), true),
            AccountMeta::new(slab, false),
        ],
        data,
    };
    let bh = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer, &admin, &new_op], bh);
    svm.send_transaction(tx).expect("real percolator accepts the operator rotation encoding");

    // ADVERSARIAL: a random key (not the asset_admin, not the current operator)
    // cannot hijack the insurance operator. The whole handoff's safety rests on
    // percolator gating authority rotations — if it didn't, anyone could seize the
    // operator and drain insurance. Pin that percolator rejects it.
    let attacker = Keypair::new();
    let attacker_target = Keypair::new();
    let mut bad = vec![65u8];
    bad.extend_from_slice(&0u16.to_le_bytes());
    bad.push(2u8); // INSURANCE_OPERATOR
    bad.extend_from_slice(attacker_target.pubkey().as_ref());
    let bad_ix = Instruction {
        program_id: perc_id(),
        accounts: vec![
            AccountMeta::new_readonly(attacker.pubkey(), true), // NOT the asset_admin/operator
            AccountMeta::new_readonly(attacker_target.pubkey(), true),
            AccountMeta::new(slab, false),
        ],
        data: bad,
    };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(&[bad_ix], Some(&payer.pubkey()), &[&payer, &attacker, &attacker_target], bh);
    assert!(
        svm.send_transaction(tx).is_err(),
        "a non-authority must not be able to hijack the insurance operator"
    );
}

// The handoff also rotates the insurance POLICY (principal-only -> surplus-only) via
// percolator UpdateInsurancePolicy (tag 33), gated on the GLOBAL marketauth. Pin
// against the real binary: (a) the encoding the twap chain uses is accepted, and
// (b) — the security boundary — a NON-marketauth cannot change the policy. Without
// (b) an attacker could set deposits_only=0, max_bps=10000 and enable draining ALL
// insurance principal.
#[test]
fn percolator_update_insurance_policy_is_marketauth_gated() {
    let mut svm = LiteSVM::new().with_compute_budget(ComputeBudget {
        compute_unit_limit: 1_400_000,
        heap_size: 256 * 1024,
        ..ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let mint_auth = Keypair::new();
    let mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());

    let admin = Keypair::new(); // marketauth
    let slab = Pubkey::new_unique();
    let init_slot = 100u64;
    let slab_data = make_live_market(&slab, &mint, &admin.pubkey(), init_slot);
    svm.set_account(
        slab,
        Account { lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0 },
    )
    .unwrap();
    svm.set_sysvar(&Clock { slot: init_slot, unix_timestamp: 100, ..Clock::default() });

    // UpdateInsurancePolicy(max_bps=10000, deposits_only=1, cooldown=0): tag 33.
    let policy_data = || {
        let mut d = vec![33u8];
        d.extend_from_slice(&10_000u16.to_le_bytes());
        d.push(1u8);
        d.extend_from_slice(&0u64.to_le_bytes());
        d
    };

    // Positive: the marketauth sets the policy — encoding accepted.
    let ok = Instruction {
        program_id: perc_id(),
        accounts: vec![
            AccountMeta::new_readonly(admin.pubkey(), true),
            AccountMeta::new(slab, false),
        ],
        data: policy_data(),
    };
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&[ok], Some(&payer.pubkey()), &[&payer, &admin], bh))
        .expect("marketauth can set the insurance policy");

    // ADVERSARIAL: a non-marketauth cannot change the policy.
    let attacker = Keypair::new();
    let bad = Instruction {
        program_id: perc_id(),
        accounts: vec![
            AccountMeta::new_readonly(attacker.pubkey(), true),
            AccountMeta::new(slab, false),
        ],
        data: policy_data(),
    };
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    assert!(
        svm.send_transaction(Transaction::new_signed_with_payer(&[bad], Some(&payer.pubkey()), &[&payer, &attacker], bh)).is_err(),
        "a non-marketauth must not be able to change the insurance policy"
    );
}

// Front-run griefing DOS (finding M2): register_proposal is otherwise permissionless,
// so an attacker could register a creator's partially-built proposal, freezing the
// (entry_count,total_amount) snapshot; the creator's next append would then make the
// live proposal mismatch the snapshot and trigger would reject it forever. Fixed by
// requiring the registrant to be the proposal's creator. Here a non-creator is
// rejected and the creator succeeds.
#[test]
fn only_the_proposal_creator_can_register_it() {
    let mut env = Env::new();
    env.init_insurance_pool();
    let ve = setup_vote(&mut env);
    let dist_config = ve.dist_config;

    let creator = Keypair::new();
    env.svm.airdrop(&creator.pubkey(), 10_000_000_000).unwrap();
    let id = 1u64;
    let dist_proposal =
        Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &id.to_le_bytes()], &dist_id()).0;
    let mut cd = vec![1u8];
    cd.extend_from_slice(&id.to_le_bytes());
    cd.extend_from_slice(&4u32.to_le_bytes());
    env.send(&[Instruction {
        program_id: dist_id(),
        accounts: vec![
            AccountMeta::new(creator.pubkey(), true),
            AccountMeta::new_readonly(dist_config, false),
            AccountMeta::new(dist_proposal, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data: cd,
    }], &[&creator]).expect("creator creates proposal");
    let dest = Pubkey::new_unique();
    let mut ad = vec![2u8];
    ad.extend_from_slice(&1u32.to_le_bytes());
    ad.extend_from_slice(dest.as_ref());
    ad.extend_from_slice(&100u64.to_le_bytes());
    env.send(&[Instruction {
        program_id: dist_id(),
        accounts: vec![
            AccountMeta::new(creator.pubkey(), true),
            AccountMeta::new_readonly(dist_config, false),
            AccountMeta::new(dist_proposal, false),
        ],
        data: ad,
    }], &[&creator]).expect("creator appends");

    let gv_proposal =
        Pubkey::find_program_address(&[b"gv_proposal", ve.gv_config.as_ref(), dist_proposal.as_ref()], &gv_id()).0;
    let register = |payer: Pubkey| Instruction {
        program_id: gv_id(),
        accounts: vec![
            AccountMeta::new(payer, true),
            AccountMeta::new_readonly(ve.gv_config, false),
            AccountMeta::new(gv_proposal, false),
            AccountMeta::new_readonly(dist_proposal, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data: vec![2u8],
    };

    // ATTACKER (env.payer, not the creator) cannot front-register.
    assert!(
        env.send(&[register(env.payer.pubkey())], &[]).is_err(),
        "a non-creator must not be able to register the proposal"
    );
    // The creator can register their own.
    env.send(&[register(creator.pubkey())], &[&creator]).expect("creator registers");
    assert!(env.svm.get_account(&gv_proposal).is_some_and(|a| !a.data.is_empty()), "gv_proposal created by the creator");
}

// Finding Q regression: init_insurance_pool is permissionless, so before the pool PDA
// committed to its market binding an attacker could grab the genesis pool PDA
// (= f(COIN_mint, asset 0)) FIRST, bound to a percolator market THEY control, with
// vote_authority set to the predictable real gv config PDA — passing the gv binding
// check and routing every depositor's principal into the attacker's market (LOF). Now
// the pool PDA commits to (mint, asset_id, market_slab, percolator_program), so an
// attacker's pool lands at a DIFFERENT address and the genesis pool PDA — bound to the
// real market — stays free and untouched.
#[test]
fn init_insurance_pool_cannot_be_squatted_to_misdirect_the_genesis_pool() {
    let mut env = Env::new();

    // The attacker stands up their OWN percolator market with a canonical insurance
    // vault for the same COIN mint (marketauth is irrelevant to pool init).
    let attacker_slab = Pubkey::new_unique();
    let attacker_marketauth = Pubkey::new_unique();
    let slab_data = make_live_market(&attacker_slab, &env.mint, &attacker_marketauth, 100);
    env.svm.set_account(attacker_slab, Account {
        lamports: 1_000_000_000, data: slab_data, owner: perc_id(), executable: false, rent_epoch: 0,
    }).unwrap();
    let attacker_vault_authority =
        Pubkey::find_program_address(&[b"vault", attacker_slab.as_ref()], &perc_id()).0;
    let attacker_vault = Pubkey::find_program_address(
        &[attacker_vault_authority.as_ref(), spl_token::ID.as_ref(), env.mint.as_ref()],
        &ATA_PROGRAM_ID,
    ).0;
    env.svm.set_account(attacker_vault, Account {
        lamports: 1_000_000, data: token_account_data(&env.mint, &attacker_vault_authority, 0),
        owner: spl_token::ID, executable: false, rent_epoch: 0,
    }).unwrap();

    // The attacker's pool PDA is bound to THEIR market — a different address from the
    // genesis pool (env.pool), which is bound to the real market (env.slab).
    let attacker_pool = Pubkey::find_program_address(
        &[b"subledger_pool", env.mint.as_ref(), &ASSET_ID.to_le_bytes(), attacker_slab.as_ref(), perc_id().as_ref()],
        &sub_id(),
    ).0;
    assert_ne!(attacker_pool, env.pool, "the market binding is part of the pool PDA");

    // The attacker CAN init their own pool (init is permissionless) — but only at THEIR
    // PDA, bound to THEIR market. It does NOT touch the genesis pool PDA.
    let mut data = vec![3u8]; // IX_INIT_INSURANCE_POOL
    data.extend_from_slice(&ASSET_ID.to_le_bytes());
    data.push(POLICY_PRINCIPAL);
    let squat = Instruction {
        program_id: sub_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(env.mint, false),
            AccountMeta::new(attacker_pool, false),
            AccountMeta::new_readonly(attacker_vault, false),
            AccountMeta::new_readonly(attacker_slab, false),
            AccountMeta::new_readonly(perc_id(), false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
            AccountMeta::new_readonly(gv_config_pda(&env.coin_mint, &env.pool), false),
        ],
        data,
    };
    env.send(&[squat], &[]).expect("attacker may init their own pool, but at their own PDA");
    assert!(env.svm.get_account(&env.pool).map_or(true, |a| a.data.is_empty()), "genesis pool PDA untouched");

    // THE GENESIS POOL STILL INITS: the squat did not block it, and it binds the REAL
    // market — depositor principal can only ever route into the real market.
    env.init_insurance_pool();
    let pool_acc = env.svm.get_account(&env.pool).unwrap();
    let bound_market = Pubkey::new_from_array(pool_acc.data[96..128].try_into().unwrap());
    assert_eq!(bound_market, env.slab, "genesis pool binds the REAL market, not the attacker's");
}

// PHANTOM-CAPITAL VOTE (Sybil-resistance core): vote weight must reflect capital GENUINELY at risk.
// Probe: deposit P, back a proposal, retract, WITHDRAW the capital, then back AGAIN — trying to vote
// with principal already pulled out. genesis-vote `read_sub_position` reads `principal` and does NOT
// check a withdrawn flag, so IF withdraw left `principal` intact (only flipping a flag) the re-vote
// would award full weight for capital no longer at risk, while the quorum denominator (live
// outstanding) had dropped — a free, denominator-shrinking Sybil vote. BLOCKED:
// `process_insurance_withdraw` DECREMENTS `position.principal -= amount`, so a full exit zeroes the
// live principal; the re-vote computes weight 0 and is rejected. (A partial exit leaves only the
// remaining at-risk principal as weight — also correct.)
#[test]
fn cannot_vote_with_a_withdrawn_position() {
    let mut env = Env::new();
    env.init_insurance_pool();
    let ve = setup_vote(&mut env);

    let amount = 1_000_000u64;
    let (alice, alice_ata) = new_depositor(&mut env, amount);
    let pool = env.pool;
    let holding = create_holding(&mut env, &pool);
    env.insurance_deposit(&alice, &alice_ata, &holding, amount).expect("deposit");
    let dest = Pubkey::new_unique();
    let (_dist_proposal, gv_proposal) = create_and_register_proposal(&mut env, &ve, 1, &dest);

    env.warp_slot(1124);
    gv_vote(&mut env, &ve, &alice, &gv_proposal, 1).expect("first vote (real capital at risk)");
    gv_vote(&mut env, &ve, &alice, &gv_proposal, 2).expect("retract to unlock");
    env.insurance_withdraw(&alice, &alice_ata, &holding, &alice, amount).expect("withdraw — capital returned");
    assert_eq!(env.token_amount(&alice_ata), amount, "alice got her capital back");
    assert_eq!(env.pool_outstanding(), 0, "outstanding no longer counts the withdrawn principal");

    // The withdrawal zeroed the LIVE principal, so there is no phantom capital to vote with.
    let (live_principal, _start, withdrawn) = env.read_position(&alice.pubkey());
    assert_eq!(live_principal, 0, "full withdraw zeroes the position's live principal");
    assert!(withdrawn, "position marked withdrawn");

    // ATTACK: vote AGAIN with the now-empty position — rejected (weight 0).
    assert!(gv_vote(&mut env, &ve, &alice, &gv_proposal, 1).is_err(),
        "voting with a fully-withdrawn (zero-principal) position must be rejected");
}
