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

        // The subledger insurance pool PDA: asset-0 insurance authority + operator.
        let pool = Pubkey::find_program_address(
            &[b"subledger_pool", mint.as_ref(), &ASSET_ID.to_le_bytes()],
            &sub_id(),
        )
        .0;

        // Build the real Live market-0 slab with marketauth = pool PDA and the
        // deposits-only principal-recovery insurance policy.
        let slab = Pubkey::new_unique();
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
                AccountMeta::new_readonly(gv_config_pda(&self.coin_mint), false),
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
}

fn gv_config_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"gv_config", mint.as_ref()], &gv_id()).0
}
fn dist_config_pda(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"dist_config", mint.as_ref()], &dist_id()).0
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
    let gv_config = gv_config_pda(&coin_mint);
    let dist_config = dist_config_pda(&coin_mint);

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

    VoteEnv { gv_config, dist_config }
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

// Venue haircut + surplus behaviour of the insurance exit, against real percolator.
//
// SURPLUS: correctly EXCLUDED. percolator caps each WithdrawInsuranceLimited to
// `insurance*max_bps/1e4` then `min(deposit_remaining)`; with deposits_only=1 the cap
// is the deposited principal, so market profit/surplus is never withdrawable here.
//
// HAIRCUT: NOT pro-rata — FIRST-COME. The cap tracks the LIVE insurance/vault, so
// under an impairment (a venue loss that drops the vault below total deposited
// principal) an early depositor withdraws their FULL principal and drains the pool,
// stranding a later depositor. The subledger requests the full `amount` and computes
// no health-ratio haircut. This pins that behaviour so any future pro-rata fix is a
// deliberate, tested change.
#[test]
fn impaired_insurance_exit_is_first_come_not_pro_rata() {
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

    // Simulate a 50% venue loss: the insurance vault now holds only half (the slab's
    // internal counters are unchanged, so the cap is bounded by `amount <= vault`).
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

    // Alice (early) withdraws her FULL principal — no haircut — and drains the pool.
    env.insurance_withdraw(&alice, &alice_ata, &a_hold, &alice, amount).expect("alice exits whole");
    assert_eq!(env.token_amount(&alice_ata), amount, "early depositor got FULL principal, no haircut");
    assert_eq!(env.token_amount(&env.perc_vault.clone()), 0, "impaired pool drained by the first exit");

    // Bob (late) is stranded — nothing left, and no pro-rata share was reserved.
    assert!(
        env.insurance_withdraw(&bob, &bob_ata, &b_hold, &bob, amount).is_err(),
        "late depositor cannot exit: first-come, not pro-rata"
    );
    assert_eq!(env.token_amount(&bob_ata), 0, "stranded depositor received nothing");
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
    let foreign_config =
        Pubkey::find_program_address(&[b"dist_config", foreign_mint.as_ref()], &dist_id()).0;
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
            AccountMeta::new_readonly(Pubkey::new_unique(), false), // some authority
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
            AccountMeta::new_readonly(gv_config_pda(&env.coin_mint), false),
        ],
        data,
    };
    let res = env.send(&[ix], &[]);
    assert!(res.is_err(), "init must reject a non-canonical vault");

    // The pool account was never created, so the canonical path still works.
    assert!(env.svm.get_account(&env.pool).map_or(true, |a| a.data.is_empty()));
    env.init_insurance_pool();
}
