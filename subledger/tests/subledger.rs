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
    Pubkey::find_program_address(
        &[b"subledger_pool", mint.as_ref(), &asset_id.to_le_bytes()],
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
