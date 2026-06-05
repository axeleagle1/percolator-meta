//! End-to-end litesvm tests for the subledger program: real SPL token vault,
//! PDA-signed withdrawals, and both exit policies (principal / with-surplus),
//! including the impaired-pool pro-rata path.

use litesvm::LiteSVM;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    program_pack::Pack,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};

fn program_id() -> Pubkey {
    subledger_program::id()
}

fn so_path() -> String {
    // workspace target/deploy/subledger_program.so
    format!(
        "{}/../target/deploy/subledger_program.so",
        env!("CARGO_MANIFEST_DIR")
    )
}

struct Env {
    svm: LiteSVM,
    payer: Keypair,
    mint: Pubkey,
    mint_authority: Keypair,
}

impl Env {
    fn new() -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program_from_file(program_id(), so_path()).unwrap();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
        let mint_authority = Keypair::new();
        let mint = create_mint(&mut svm, &payer, &mint_authority.pubkey());
        Env {
            svm,
            payer,
            mint,
            mint_authority,
        }
    }

    /// Signs with the env payer (fee payer) plus any `extra` signers.
    fn send(&mut self, ixs: &[Instruction], extra: &[&Keypair]) -> Result<(), String> {
        self.svm.expire_blockhash();
        let bh = self.svm.latest_blockhash();
        let payer = clone_kp(&self.payer);
        let mut signers: Vec<&Keypair> = Vec::with_capacity(1 + extra.len());
        signers.push(&payer);
        signers.extend_from_slice(extra);
        let payer_pubkey = self.payer.pubkey();
        let tx = Transaction::new_signed_with_payer(ixs, Some(&payer_pubkey), &signers, bh);
        self.svm.send_transaction(tx).map(|_| ()).map_err(|e| format!("{:?}", e))
    }

    fn token_amount(&self, account: &Pubkey) -> u64 {
        let acc = self.svm.get_account(account).unwrap();
        spl_token::state::Account::unpack(&acc.data).unwrap().amount
    }
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
    let ix = spl_token::instruction::mint_to(&spl_token::ID, mint, dest, &authority.pubkey(), &[], amount).unwrap();
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&payer.pubkey()),
        &[payer, authority],
        svm.latest_blockhash(),
    );
    svm.send_transaction(tx).unwrap();
}

fn pool_pda(mint: &Pubkey, asset_id: u64) -> Pubkey {
    // Own-vault pools commit to the default market binding (no percolator market).
    let no_market = Pubkey::default();
    Pubkey::find_program_address(
        &[
            b"subledger_pool",
            mint.as_ref(),
            &asset_id.to_le_bytes(),
            no_market.as_ref(),
            no_market.as_ref(),
        ],
        &program_id(),
    )
    .0
}

fn position_pda(pool: &Pubkey, owner: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"subledger_position", pool.as_ref(), owner.as_ref()],
        &program_id(),
    )
    .0
}

fn init_pool_ix(env: &Env, pool: &Pubkey, vault: &Pubkey, asset_id: u64, policy: u8) -> Instruction {
    let mut data = vec![0u8]; // IX_INIT_POOL
    data.extend_from_slice(&asset_id.to_le_bytes());
    data.push(policy);
    data.push(0u8); // domain = insurance (own-vault behaviour is identical)
    Instruction {
        program_id: program_id(),
        accounts: vec![
            AccountMeta::new(env.payer.pubkey(), true),
            AccountMeta::new_readonly(env.mint, false),
            AccountMeta::new(*pool, false),
            AccountMeta::new_readonly(*vault, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data,
    }
}

fn deposit_ix(env: &Env, pool: &Pubkey, owner: &Pubkey, owner_ata: &Pubkey, vault: &Pubkey, amount: u64) -> Instruction {
    let mut data = vec![1u8]; // IX_DEPOSIT
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: program_id(),
        accounts: vec![
            AccountMeta::new(*owner, true),
            AccountMeta::new(*pool, false),
            AccountMeta::new(position_pda(pool, owner), false),
            AccountMeta::new(*owner_ata, false),
            AccountMeta::new(*vault, false),
            AccountMeta::new_readonly(spl_token::ID, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ],
        data,
    }
}

fn withdraw_ix(pool: &Pubkey, owner: &Pubkey, owner_ata: &Pubkey, vault: &Pubkey) -> Instruction {
    Instruction {
        program_id: program_id(),
        accounts: vec![
            AccountMeta::new(*owner, true),
            AccountMeta::new(*pool, false),
            AccountMeta::new(position_pda(pool, owner), false),
            AccountMeta::new(*owner_ata, false),
            AccountMeta::new(*vault, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data: vec![2u8], // IX_WITHDRAW
    }
}

/// Funds a depositor: airdrop SOL, create their ATA, mint `amount` to it.
fn new_depositor(env: &mut Env, amount: u64) -> (Keypair, Pubkey) {
    let kp = Keypair::new();
    env.svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
    let payer = clone_kp(&env.payer);
    let auth = clone_kp(&env.mint_authority);
    let mint = env.mint;
    let ata = create_token_account(&mut env.svm, &payer, &mint, &kp.pubkey());
    if amount > 0 {
        mint_to(&mut env.svm, &payer, &mint, &auth, &ata, amount);
    }
    (kp, ata)
}

fn clone_kp(kp: &Keypair) -> Keypair {
    Keypair::from_bytes(&kp.to_bytes()).unwrap()
}

#[test]
fn principal_policy_healthy_pays_principal_and_keeps_surplus() {
    let mut env = Env::new();
    let asset_id = 1;
    let pool = pool_pda(&env.mint, asset_id);
    let vault = create_token_account(&mut env.svm, &clone_kp(&env.payer), &env.mint, &pool);

    env.send(&[init_pool_ix(&env, &pool, &vault, asset_id, 0)], &[])
        .expect("init pool");

    let (alice, alice_ata) = new_depositor(&mut env, 60);
    let (bob, bob_ata) = new_depositor(&mut env, 40);
    env.send(&[deposit_ix(&env, &pool, &alice.pubkey(), &alice_ata, &vault, 60)], &[&alice]).unwrap();
    env.send(&[deposit_ix(&env, &pool, &bob.pubkey(), &bob_ata, &vault, 40)], &[&bob]).unwrap();
    assert_eq!(env.token_amount(&vault), 100, "principal deposited");

    // Simulate local fees/yield: 50 extra tokens land in the vault.
    let auth = clone_kp(&env.mint_authority);
    mint_to(&mut env.svm, &clone_kp(&env.payer), &env.mint, &auth, &vault, 50);
    assert_eq!(env.token_amount(&vault), 150);

    // Healthy (balance 150 >= outstanding 100): principal policy returns principal only.
    env.send(&[withdraw_ix(&pool, &alice.pubkey(), &alice_ata, &vault)], &[&alice]).unwrap();
    assert_eq!(env.token_amount(&alice_ata), 60, "alice gets principal, not surplus");

    env.send(&[withdraw_ix(&pool, &bob.pubkey(), &bob_ata, &vault)], &[&bob]).unwrap();
    assert_eq!(env.token_amount(&bob_ata), 40, "bob gets principal");

    // The 50 surplus stays in the pool (no further claimant under principal policy).
    assert_eq!(env.token_amount(&vault), 50, "surplus retained in pool");

    // Double-withdraw is rejected.
    assert!(env.send(&[withdraw_ix(&pool, &alice.pubkey(), &alice_ata, &vault)], &[&alice]).is_err());
}

#[test]
fn with_surplus_policy_returns_yield_pro_rata() {
    let mut env = Env::new();
    let asset_id = 2;
    let pool = pool_pda(&env.mint, asset_id);
    let vault = create_token_account(&mut env.svm, &clone_kp(&env.payer), &env.mint, &pool);

    env.send(&[init_pool_ix(&env, &pool, &vault, asset_id, 1)], &[])
        .expect("init pool");

    let (alice, alice_ata) = new_depositor(&mut env, 60);
    let (bob, bob_ata) = new_depositor(&mut env, 40);
    env.send(&[deposit_ix(&env, &pool, &alice.pubkey(), &alice_ata, &vault, 60)], &[&alice]).unwrap();
    env.send(&[deposit_ix(&env, &pool, &bob.pubkey(), &bob_ata, &vault, 40)], &[&bob]).unwrap();

    let auth = clone_kp(&env.mint_authority);
    mint_to(&mut env.svm, &clone_kp(&env.payer), &env.mint, &auth, &vault, 50); // balance 150
    assert_eq!(env.token_amount(&vault), 150);

    // With-surplus: pro-rata against the live balance. alice 150*60/100 = 90.
    env.send(&[withdraw_ix(&pool, &alice.pubkey(), &alice_ata, &vault)], &[&alice]).unwrap();
    assert_eq!(env.token_amount(&alice_ata), 90, "alice gets principal + surplus share");
    // bob now: balance 60, outstanding 40 -> 60*40/40 = 60.
    env.send(&[withdraw_ix(&pool, &bob.pubkey(), &bob_ata, &vault)], &[&bob]).unwrap();
    assert_eq!(env.token_amount(&bob_ata), 60, "bob gets the rest");
    assert_eq!(env.token_amount(&vault), 0, "surplus fully distributed");
}

fn set_token_amount(svm: &mut LiteSVM, account: &Pubkey, amount: u64) {
    let mut acc = svm.get_account(account).unwrap();
    let mut state = spl_token::state::Account::unpack(&acc.data).unwrap();
    state.amount = amount;
    spl_token::state::Account::pack(state, &mut acc.data).unwrap();
    svm.set_account(*account, acc).unwrap();
}

#[test]
fn impaired_pool_is_pro_rata_and_order_independent() {
    let mut env = Env::new();
    let asset_id = 3;
    let pool = pool_pda(&env.mint, asset_id);
    let vault = create_token_account(&mut env.svm, &clone_kp(&env.payer), &env.mint, &pool);

    env.send(&[init_pool_ix(&env, &pool, &vault, asset_id, 0)], &[])
        .expect("init pool");

    let (alice, alice_ata) = new_depositor(&mut env, 60);
    let (bob, bob_ata) = new_depositor(&mut env, 40);
    env.send(&[deposit_ix(&env, &pool, &alice.pubkey(), &alice_ata, &vault, 60)], &[&alice]).unwrap();
    env.send(&[deposit_ix(&env, &pool, &bob.pubkey(), &bob_ata, &vault, 40)], &[&bob]).unwrap();
    assert_eq!(env.token_amount(&vault), 100);

    // Impair the pool: a 50% market loss leaves only 50 in the vault against 100
    // outstanding principal.
    set_token_amount(&mut env.svm, &vault, 50);

    // Alice withdraws first: pro-rata 50 * 60 / 100 = 30 (a 50% haircut).
    env.send(&[withdraw_ix(&pool, &alice.pubkey(), &alice_ata, &vault)], &[&alice]).unwrap();
    assert_eq!(env.token_amount(&alice_ata), 30, "alice takes her pro-rata 50% haircut");

    // Bob withdraws second: full principal retired from outstanding keeps the ratio,
    // so bob gets the same 50% — 20 of 40 — order-independent, no bank run.
    env.send(&[withdraw_ix(&pool, &bob.pubkey(), &bob_ata, &vault)], &[&bob]).unwrap();
    assert_eq!(env.token_amount(&bob_ata), 20, "bob takes the same 50% haircut, not a worse one");
    assert_eq!(env.token_amount(&vault), 0, "impaired balance fully and fairly distributed");
}

#[test]
fn non_owner_cannot_withdraw_another_position() {
    let mut env = Env::new();
    let asset_id = 4;
    let pool = pool_pda(&env.mint, asset_id);
    let vault = create_token_account(&mut env.svm, &clone_kp(&env.payer), &env.mint, &pool);
    env.send(&[init_pool_ix(&env, &pool, &vault, asset_id, 0)], &[]).unwrap();

    let (alice, alice_ata) = new_depositor(&mut env, 60);
    env.send(&[deposit_ix(&env, &pool, &alice.pubkey(), &alice_ata, &vault, 60)], &[&alice]).unwrap();

    // An attacker signs and points the withdraw at alice's position PDA, paying to
    // their own ATA. The position PDA is keyed by alice's pubkey, so the attacker's
    // derived position differs and the owner check rejects it.
    let (attacker, attacker_ata) = new_depositor(&mut env, 0);
    let mut ix = withdraw_ix(&pool, &alice.pubkey(), &attacker_ata, &vault);
    ix.accounts[0] = AccountMeta::new(attacker.pubkey(), true); // attacker signs
    assert!(
        env.send(&[ix], &[&attacker]).is_err(),
        "only the position owner can withdraw it"
    );
    assert_eq!(env.token_amount(&attacker_ata), 0);
}

// Anti-theft boundary: init_pool must reject a vault that is NOT owned by the pool
// PDA. If it accepted an attacker-owned vault, the attacker could stand up a pool,
// lure a victim's deposit (tag 1 transfers owner -> pool.vault), and then drain the
// funds directly via SPL as the vault owner — while the program's withdraw (which
// signs as the pool PDA) could never move them. The vault must be pool-PDA-owned so
// only this program can move funds out.
#[test]
fn init_pool_rejects_a_vault_not_owned_by_the_pool() {
    let mut env = Env::new();
    let asset_id = 0u64;
    let pool = pool_pda(&env.mint, asset_id);

    // A vault owned by an ATTACKER rather than the pool PDA.
    let attacker = Pubkey::new_unique();
    let rogue_vault = create_token_account(&mut env.svm, &clone_kp(&env.payer), &env.mint, &attacker);
    assert!(
        env.send(&[init_pool_ix(&env, &pool, &rogue_vault, asset_id, 0)], &[]).is_err(),
        "init_pool must reject a vault not owned by the pool PDA"
    );

    // The canonical (pool-PDA-owned) vault is accepted.
    let good_vault = create_token_account(&mut env.svm, &clone_kp(&env.payer), &env.mint, &pool);
    env.send(&[init_pool_ix(&env, &pool, &good_vault, asset_id, 0)], &[])
        .expect("a pool-PDA-owned vault is accepted");
}

// CROSS-POOL DRAIN (pool-isolation half of the owner/pool guard): withdraw checks BOTH position.owner ==
// owner AND position.pool == pool_account (lib.rs process_withdraw). non_owner_cannot_withdraw_another_
// position pins the owner half; this pins the POOL half. Without it, an attacker who holds a real position
// in pool-A (their own deposit) could pass that position alongside pool-B's pool + vault and withdraw
// against pool-B — using pool-A principal to drain a DIFFERENT pool's vault (another depositor's funds).
#[test]
fn cannot_drain_a_foreign_pool_with_a_position_from_another_pool() {
    let mut env = Env::new();
    // Two independent own-vault pools (same mint, different asset_ids), each with its own vault.
    let pool_a = pool_pda(&env.mint, 1);
    let vault_a = create_token_account(&mut env.svm, &clone_kp(&env.payer), &env.mint, &pool_a);
    env.send(&[init_pool_ix(&env, &pool_a, &vault_a, 1, 0)], &[]).expect("init pool A");
    let pool_b = pool_pda(&env.mint, 2);
    let vault_b = create_token_account(&mut env.svm, &clone_kp(&env.payer), &env.mint, &pool_b);
    env.send(&[init_pool_ix(&env, &pool_b, &vault_b, 2, 0)], &[]).expect("init pool B");

    // Attacker holds a 1M position in pool-A; a victim funds pool-B with 1M.
    let (attacker, attacker_ata) = new_depositor(&mut env, 1_000_000);
    env.send(&[deposit_ix(&env, &pool_a, &attacker.pubkey(), &attacker_ata, &vault_a, 1_000_000)], &[&attacker]).unwrap();
    let (victim, victim_ata) = new_depositor(&mut env, 1_000_000);
    env.send(&[deposit_ix(&env, &pool_b, &victim.pubkey(), &victim_ata, &vault_b, 1_000_000)], &[&victim]).unwrap();
    assert_eq!(env.token_amount(&vault_b), 1_000_000, "victim's pool-B vault funded");

    // ATTACK: withdraw against pool-B + vault-B, but pass the attacker's pool-A POSITION (principal 1M).
    let attack = Instruction {
        program_id: program_id(),
        accounts: vec![
            AccountMeta::new(attacker.pubkey(), true),
            AccountMeta::new(pool_b, false),
            AccountMeta::new(position_pda(&pool_a, &attacker.pubkey()), false), // pool-A position
            AccountMeta::new(attacker_ata, false),
            AccountMeta::new(vault_b, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
        data: vec![2u8], // IX_WITHDRAW
    };
    assert!(env.send(&[attack], &[&attacker]).is_err(),
        "withdraw must reject a position bound to a DIFFERENT pool (cross-pool drain)");
    assert_eq!(env.token_amount(&vault_b), 1_000_000, "pool-B vault untouched — victim's funds safe");
    assert_eq!(env.token_amount(&attacker_ata), 0, "attacker gained nothing from the cross-pool attempt");

    // The attacker's own pool-A position is intact: they can still exit pool-A for exactly their principal.
    env.send(&[withdraw_ix(&pool_a, &attacker.pubkey(), &attacker_ata, &vault_a)], &[&attacker]).expect("attacker exits their OWN pool A");
    assert_eq!(env.token_amount(&attacker_ata), 1_000_000, "attacker recovers only their own pool-A principal, never pool-B's");
}
