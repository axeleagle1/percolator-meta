//! [branch-only, DO NOT PUSH] e2e: residual-distributor (deterministic decider) drives the
//! real `distribution` program. Proves the verify-then-seal seam: register → crystallize
//! (snapshot-delta of a mock percolator backing-ledger) → cranker builds the deterministic
//! proposal → decider seals via CPI → recipient claims. Mirrors what genesis-vote does, but
//! the winner is computed from residual points, not a vote.

use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    clock::Clock,
    instruction::{AccountMeta, Instruction},
    program_pack::Pack,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};
use spl_token::instruction::AuthorityType;

fn rd_id() -> Pubkey {
    Pubkey::from(residual_distributor::ID)
}
fn dist_id() -> Pubkey {
    // distribution_program declared id.
    Pubkey::try_from("D1str1but1on11111111111111111111111111111111").unwrap()
}
fn rd_so() -> String {
    format!("{}/../target/deploy/residual_distributor.so", env!("CARGO_MANIFEST_DIR"))
}
fn dist_so() -> String {
    format!("{}/../target/deploy/distribution_program.so", env!("CARGO_MANIFEST_DIR"))
}

fn create_mint(svm: &mut LiteSVM, payer: &Keypair, authority: &Pubkey) -> Pubkey {
    let mint = Keypair::new();
    let rent = svm.minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN);
    let ixs = [
        system_instruction::create_account(&payer.pubkey(), &mint.pubkey(), rent, spl_token::state::Mint::LEN as u64, &spl_token::ID),
        spl_token::instruction::initialize_mint(&spl_token::ID, &mint.pubkey(), authority, None, 6).unwrap(),
    ];
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[payer, &mint], svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();
    mint.pubkey()
}
fn create_token_account(svm: &mut LiteSVM, payer: &Keypair, mint: &Pubkey, owner: &Pubkey) -> Pubkey {
    let acc = Keypair::new();
    let rent = svm.minimum_balance_for_rent_exemption(spl_token::state::Account::LEN);
    let ixs = [
        system_instruction::create_account(&payer.pubkey(), &acc.pubkey(), rent, spl_token::state::Account::LEN as u64, &spl_token::ID),
        spl_token::instruction::initialize_account(&spl_token::ID, &acc.pubkey(), mint, owner).unwrap(),
    ];
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[payer, &acc], svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();
    acc.pubkey()
}
fn mint_to(svm: &mut LiteSVM, payer: &Keypair, mint: &Pubkey, authority: &Keypair, dest: &Pubkey, amount: u64) {
    let ix = spl_token::instruction::mint_to(&spl_token::ID, mint, dest, &authority.pubkey(), &[], amount).unwrap();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer, authority], svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();
}
fn revoke_mint(svm: &mut LiteSVM, payer: &Keypair, mint: &Pubkey, authority: &Keypair) {
    let ix = spl_token::instruction::set_authority(&spl_token::ID, mint, None, AuthorityType::MintTokens, &authority.pubkey(), &[]).unwrap();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer, authority], svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();
}
fn token_amount(svm: &LiteSVM, acc: &Pubkey) -> u64 {
    spl_token::state::Account::unpack(&svm.get_account(acc).unwrap().data).unwrap().amount
}
fn set_slot(svm: &mut LiteSVM, slot: u64) {
    svm.set_sysvar(&Clock { slot, ..Default::default() });
}
fn send(svm: &mut LiteSVM, payer: &Keypair, ixs: &[Instruction], extra: &[&Keypair]) -> Result<(), String> {
    svm.expire_blockhash();
    let bh = svm.latest_blockhash();
    let mut signers: Vec<&Keypair> = vec![payer];
    signers.extend_from_slice(extra);
    let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), &signers, bh);
    svm.send_transaction(tx).map(|_| ()).map_err(|e| format!("{:?}", e))
}

// Mock percolator BackingDomainLedger: the fields the decider snapshots, at the REAL
// pinned absolute offsets (HEADER_LEN 16 + within-struct). residual_received =
// cumulative_loss@176, total_earnings@128. (tests/offsets.rs pins these vs the real struct.)
const OFF_TOTAL_EARNINGS: usize = 128;
const OFF_CUMULATIVE_LOSS: usize = 176;
fn set_backing_ledger(svm: &mut LiteSVM, key: &Pubkey, perc: &Pubkey, authority: &Pubkey, loss: u128, earnings: u128) {
    let mut data = vec![0u8; 240]; // HEADER_LEN(16) + struct size(224)
    // market_group @ 16 left default(0); config.market_group is also default in these tests, so the
    // residual scoping check is 0==0 (the HI/HN test sets a non-zero pair to exercise rejection).
    // authority @ HEADER_LEN(16) + offset_of(market_group[32]) = 48 (pinned by offsets.rs).
    data[16 + 32..16 + 64].copy_from_slice(authority.as_ref());
    data[OFF_TOTAL_EARNINGS..OFF_TOTAL_EARNINGS + 16].copy_from_slice(&earnings.to_le_bytes());
    data[OFF_CUMULATIVE_LOSS..OFF_CUMULATIVE_LOSS + 16].copy_from_slice(&loss.to_le_bytes());
    svm.set_account(*key, Account { lamports: 1_000_000_000, data, owner: *perc, executable: false, rent_epoch: 0 }).unwrap();
}

#[test]
fn deterministic_decider_seals_a_real_distribution() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    svm.add_program_from_file(dist_id(), dist_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 900_000u64;
    let fee_bps = 80u16;
    let emission_end = 2_000u64;
    let stub_perc = Pubkey::new_unique(); // mock percolator program (account owner only)

    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());

    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let dist_vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &dist_vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    // distribution InitConfig: authority = rd_config (the decider).
    let mut d = vec![0u8];
    d.extend_from_slice(&1_000_000u64.to_le_bytes()); // claim_window
    d.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false),
        AccountMeta::new(dist_config, false), AccountMeta::new_readonly(dist_vault, false),
        AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("dist init");

    // residual-distributor Init. insurance_bps = 0 (this test exercises the residual cohort only).
    let mut d = vec![0u8];
    d.extend_from_slice(&supply.to_le_bytes());
    d.extend_from_slice(&fee_bps.to_le_bytes());
    d.extend_from_slice(&emission_end.to_le_bytes());
    d.extend_from_slice(&0u16.to_le_bytes()); // insurance_bps
    let stub_sub = Pubkey::new_unique();
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false),
        AccountMeta::new_readonly(dist_id(), false), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    // Two backers; each has its own backing-ledger account.
    let alice = Keypair::new();
    let bob = Keypair::new();
    let a_ledger = Pubkey::new_unique();
    let b_ledger = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 0, 0);
    set_backing_ledger(&mut svm, &b_ledger, &stub_perc, &bob.pubkey(), 0, 0);

    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    let b_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), bob.pubkey().as_ref()], &rd_id()).0;
    for (owner, ledger, stake) in [(&alice, &a_ledger, &a_stake), (&bob, &b_ledger, &b_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false),
            AccountMeta::new_readonly(owner.pubkey(), true), AccountMeta::new_readonly(owner.pubkey(), false),
            AccountMeta::new_readonly(*ledger, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 0u8] }], &[owner]).expect("register_start");
    }

    // Time passes (hold=1000, floor_log2=9); alice absorbs 1000 loss, bob 500, both with 1e6 fees.
    set_slot(&mut svm, 1_100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 1_000, 1_000_000);
    set_backing_ledger(&mut svm, &b_ledger, &stub_perc, &bob.pubkey(), 500, 1_000_000);
    for (ledger, stake) in [(&a_ledger, &a_stake), (&b_ledger, &b_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false),
            AccountMeta::new(*stake, false), AccountMeta::new_readonly(*ledger, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }
    // eligible = min(loss, fee*10000/80): alice 1000, bob 500. points = eligible*9.
    // amounts = supply * points / total: alice 900000*9000/13500 = 600000, bob = 300000.

    // Emission ends; a cranker builds the deterministic proposal.
    set_slot(&mut svm, 2_001);
    let proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &1u64.to_le_bytes()], &dist_id()).0;
    let mut cp = vec![1u8];
    cp.extend_from_slice(&1u64.to_le_bytes()); // id
    cp.extend_from_slice(&2u32.to_le_bytes()); // capacity
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new(proposal, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: cp }], &[]).expect("create_proposal");

    let mut ae = vec![2u8];
    ae.extend_from_slice(&2u32.to_le_bytes());
    ae.extend_from_slice(alice.pubkey().as_ref());
    ae.extend_from_slice(&600_000u64.to_le_bytes());
    ae.extend_from_slice(bob.pubkey().as_ref());
    ae.extend_from_slice(&300_000u64.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new(proposal, false),
    ], data: ae }], &[]).expect("append_entries");

    // The decider verifies every entry against the on-chain PointStakes and seals.
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false),
        AccountMeta::new_readonly(dist_id(), false), AccountMeta::new(dist_config, false),
        AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(a_stake, false), AccountMeta::new_readonly(b_stake, false),
    ], data: vec![3u8] }], &[]).expect("rd seal");

    // distribution is now sealed to THIS proposal — alice claims her deterministic share.
    // (claim itself enforces is_sealed() && sealed_proposal == proposal, so a successful
    // claim is proof the decider's seal CPI landed.)
    let alice_ata = create_token_account(&mut svm, &payer, &coin_mint, &alice.pubkey());
    let mut cl = vec![4u8];
    cl.extend_from_slice(&0u32.to_le_bytes()); // index 0 = alice
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new(proposal, false), AccountMeta::new(dist_vault, false),
        AccountMeta::new(alice_ata, false), AccountMeta::new_readonly(spl_token::ID, false),
    ], data: cl }], &[&alice]).expect("alice claim");
    assert_eq!(token_amount(&svm, &alice_ata), 600_000, "alice claimed her deterministic 2/3 share");
}

// The seal is GATED on seal_slot >= emission_end_slot (lib.rs:670-671): a genesis cannot be
// FINALIZED before its emission period completes, so every backer's tenure accrues to the same
// post-emission point and no one can crank an early, unfair distribution. The happy path always
// warps past emission_end, so the REJECT side of that gate had no explicit pin. Here we build a
// valid, fully-crystallized proposal and attempt the SAME seal twice: once before emission_end
// (must reject) and once after (must succeed). Because this is a residual-only genesis
// (insurance_bps=0), the residual amounts come from the crystallized stake.points and are
// seal_slot-INDEPENDENT (lib.rs:712/797), so the ONLY thing that differs between the two attempts
// is the emission_end gate — proving the rejection is the gate and nothing else (non-tautological).
#[test]
fn seal_is_rejected_before_emission_end_then_succeeds_after() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    svm.add_program_from_file(dist_id(), dist_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 900_000u64;
    let fee_bps = 80u16;
    let emission_end = 2_000u64;
    let stub_perc = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());

    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let dist_vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &dist_vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    let mut d = vec![0u8];
    d.extend_from_slice(&1_000_000u64.to_le_bytes());
    d.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false),
        AccountMeta::new(dist_config, false), AccountMeta::new_readonly(dist_vault, false),
        AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("dist init");

    let mut d = vec![0u8];
    d.extend_from_slice(&supply.to_le_bytes());
    d.extend_from_slice(&fee_bps.to_le_bytes());
    d.extend_from_slice(&emission_end.to_le_bytes());
    d.extend_from_slice(&0u16.to_le_bytes()); // insurance_bps = 0 -> residual-only, seal_slot-independent amounts
    let stub_sub = Pubkey::new_unique();
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false),
        AccountMeta::new_readonly(dist_id(), false), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let alice = Keypair::new();
    let bob = Keypair::new();
    let a_ledger = Pubkey::new_unique();
    let b_ledger = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 0, 0);
    set_backing_ledger(&mut svm, &b_ledger, &stub_perc, &bob.pubkey(), 0, 0);
    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    let b_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), bob.pubkey().as_ref()], &rd_id()).0;
    for (owner, ledger, stake) in [(&alice, &a_ledger, &a_stake), (&bob, &b_ledger, &b_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false),
            AccountMeta::new_readonly(owner.pubkey(), true), AccountMeta::new_readonly(owner.pubkey(), false),
            AccountMeta::new_readonly(*ledger, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 0u8] }], &[owner]).expect("register_start");
    }

    set_slot(&mut svm, 1_100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 1_000, 1_000_000);
    set_backing_ledger(&mut svm, &b_ledger, &stub_perc, &bob.pubkey(), 500, 1_000_000);
    for (ledger, stake) in [(&a_ledger, &a_stake), (&b_ledger, &b_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false),
            AccountMeta::new(*stake, false), AccountMeta::new_readonly(*ledger, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }

    // Build the deterministic proposal (create + append are NOT emission-gated).
    let proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &1u64.to_le_bytes()], &dist_id()).0;
    let mut cp = vec![1u8];
    cp.extend_from_slice(&1u64.to_le_bytes());
    cp.extend_from_slice(&2u32.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new(proposal, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: cp }], &[]).expect("create_proposal");
    let mut ae = vec![2u8];
    ae.extend_from_slice(&2u32.to_le_bytes());
    ae.extend_from_slice(alice.pubkey().as_ref());
    ae.extend_from_slice(&600_000u64.to_le_bytes());
    ae.extend_from_slice(bob.pubkey().as_ref());
    ae.extend_from_slice(&300_000u64.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new(proposal, false),
    ], data: ae }], &[]).expect("append_entries");

    // The seal instruction is identical in both attempts; only the slot differs.
    let seal_ix = Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false),
        AccountMeta::new_readonly(dist_id(), false), AccountMeta::new(dist_config, false),
        AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(a_stake, false), AccountMeta::new_readonly(b_stake, false),
    ], data: vec![3u8] };

    // (1) BEFORE emission_end (slot 1_500 < 2_000): the gate must reject.
    set_slot(&mut svm, 1_500);
    let early = send(&mut svm, &payer, &[seal_ix.clone()], &[]);
    assert!(early.is_err(), "seal before emission_end must be rejected by the emission gate");

    // (2) AFTER emission_end (slot 2_001): the SAME proposal seals — proving the only blocker was
    // the gate, not the proposal contents.
    set_slot(&mut svm, 2_001);
    send(&mut svm, &payer, &[seal_ix], &[]).expect("seal after emission_end must succeed");

    // A successful claim proves the decider's seal CPI landed.
    let alice_ata = create_token_account(&mut svm, &payer, &coin_mint, &alice.pubkey());
    let mut cl = vec![4u8];
    cl.extend_from_slice(&0u32.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(alice.pubkey(), true), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new(proposal, false), AccountMeta::new(dist_vault, false),
        AccountMeta::new(alice_ata, false), AccountMeta::new_readonly(spl_token::ID, false),
    ], data: cl }], &[&alice]).expect("alice claim after a valid late seal");
    assert_eq!(token_amount(&svm, &alice_ata), 600_000, "alice's share is identical regardless of WHEN the gate let the seal through");
}

// TYPE-COSPLAY on the backing-ledger account. A RESIDUAL stake pays from stake.points with NO
// live-position cap, and those points are derived from the linked account's loss/earnings bytes — so
// if an attacker could fabricate a backing-ledger account they'd mint COIN against numbers they
// chose. The defense is the OWNER check (lib.rs:493): a residual backing_ledger MUST be owned by
// config.percolator_program, i.e. authored by the real percolator. Here we hand register a
// PERFECTLY-shaped residual ledger (authority = the attacker so the GY bind at :499 passes,
// market_group = default so the scope check at :507 passes) that is owned by the WRONG program
// (stub_sub, not stub_perc). Every check except the owner check passes, so :493 is the SOLE blocker
// — a clean isolation (a non-percolator-owned counterfeit ledger cannot farm residual COIN).
#[test]
fn register_rejects_a_residual_ledger_not_owned_by_the_percolator_program() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    svm.add_program_from_file(dist_id(), dist_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 900_000u64;
    let stub_perc = Pubkey::new_unique(); // config.percolator_program — RESIDUAL ledgers must be owned by THIS
    let stub_sub = Pubkey::new_unique();  // config.subledger_program — the insurance position IS owned by this
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let dist_vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &dist_vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    let mut d = vec![0u8];
    d.extend_from_slice(&1_000_000u64.to_le_bytes());
    d.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false),
        AccountMeta::new(dist_config, false), AccountMeta::new_readonly(dist_vault, false),
        AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("dist init");

    let mut d = vec![0u8];
    d.extend_from_slice(&supply.to_le_bytes());
    d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); // emission_end
    d.extend_from_slice(&0u16.to_le_bytes());      // insurance_bps = 0 (market_group left UNSCOPED -> :493 is sole guard)
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false),
        AccountMeta::new_readonly(dist_id(), false), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    // A counterfeit backing ledger: perfectly residual-shaped (authority = attacker so :499 passes,
    // market_group = default(0) so :507 passes) but OWNED BY stub_sub (a non-percolator program).
    let attacker = Keypair::new();
    let counterfeit = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &counterfeit, &stub_sub, &attacker.pubkey(), 1_000, 1_000_000);

    // Attacker registers the counterfeit into the RESIDUAL cohort (data byte 0).
    let stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), attacker.pubkey().as_ref()], &rd_id()).0;
    let res = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false),
        AccountMeta::new_readonly(attacker.pubkey(), true), AccountMeta::new_readonly(attacker.pubkey(), false),
        AccountMeta::new_readonly(counterfeit, false), AccountMeta::new(stake, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: vec![1u8, 0u8] }], &[&attacker]);
    assert!(res.is_err(), "a counterfeit backing ledger not owned by config.percolator_program must NOT register into the residual cohort (owner type-distinguisher, lib.rs:493)");
}

// ANTI-REDIRECT — the core theft guarantee of verify-then-seal. The seal binds each proposal entry
// to BOTH the stake's bound recipient AND the deterministic amount (lib.rs:743:
// `entry_recipient != stake.recipient || entry_amount != want`). The amount half is pinned by the
// completeness/duplicate/forfeiture tests; the RECIPIENT half — that a malicious cranker cannot pay
// an honest backer's CORRECT amount to THEMSELVES — had no dedicated pin. Here the proposal pays
// alice's exact 600k share but names the ATTACKER as the recipient of entry 0. The amount is right,
// so the amount half of :743 passes and the recipient half is the SOLE blocker (clean isolation).
#[test]
fn seal_rejects_redirecting_an_honest_backers_share_to_the_cranker() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    svm.add_program_from_file(dist_id(), dist_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 900_000u64;
    let emission_end = 2_000u64;
    let stub_perc = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let dist_vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &dist_vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    let mut d = vec![0u8];
    d.extend_from_slice(&1_000_000u64.to_le_bytes());
    d.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false),
        AccountMeta::new(dist_config, false), AccountMeta::new_readonly(dist_vault, false),
        AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("dist init");

    let mut d = vec![0u8];
    d.extend_from_slice(&supply.to_le_bytes());
    d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&emission_end.to_le_bytes());
    d.extend_from_slice(&0u16.to_le_bytes());
    let stub_sub = Pubkey::new_unique();
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false),
        AccountMeta::new_readonly(dist_id(), false), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let alice = Keypair::new();
    let bob = Keypair::new();
    let attacker = Keypair::new();
    let a_ledger = Pubkey::new_unique();
    let b_ledger = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 0, 0);
    set_backing_ledger(&mut svm, &b_ledger, &stub_perc, &bob.pubkey(), 0, 0);
    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    let b_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), bob.pubkey().as_ref()], &rd_id()).0;
    for (owner, ledger, stake) in [(&alice, &a_ledger, &a_stake), (&bob, &b_ledger, &b_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false),
            AccountMeta::new_readonly(owner.pubkey(), true), AccountMeta::new_readonly(owner.pubkey(), false),
            AccountMeta::new_readonly(*ledger, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 0u8] }], &[owner]).expect("register_start");
    }
    set_slot(&mut svm, 1_100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 1_000, 1_000_000);
    set_backing_ledger(&mut svm, &b_ledger, &stub_perc, &bob.pubkey(), 500, 1_000_000);
    for (ledger, stake) in [(&a_ledger, &a_stake), (&b_ledger, &b_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false),
            AccountMeta::new(*stake, false), AccountMeta::new_readonly(*ledger, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }

    set_slot(&mut svm, 2_001);
    let proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &1u64.to_le_bytes()], &dist_id()).0;
    let mut cp = vec![1u8];
    cp.extend_from_slice(&1u64.to_le_bytes());
    cp.extend_from_slice(&2u32.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new(proposal, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: cp }], &[]).expect("create_proposal");

    // The POISONED proposal: entry 0 carries alice's EXACT 600k amount but names the ATTACKER.
    let mut ae = vec![2u8];
    ae.extend_from_slice(&2u32.to_le_bytes());
    ae.extend_from_slice(attacker.pubkey().as_ref()); // <-- redirect: should be alice
    ae.extend_from_slice(&600_000u64.to_le_bytes());  // <-- her exact deterministic share
    ae.extend_from_slice(bob.pubkey().as_ref());
    ae.extend_from_slice(&300_000u64.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new(proposal, false),
    ], data: ae }], &[]).expect("append_entries");

    // The decider must REFUSE: entry 0's recipient (attacker) != a_stake.recipient (alice).
    let res = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false),
        AccountMeta::new_readonly(dist_id(), false), AccountMeta::new(dist_config, false),
        AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(a_stake, false), AccountMeta::new_readonly(b_stake, false),
    ], data: vec![3u8] }], &[]);
    assert!(res.is_err(), "seal must reject a proposal that redirects alice's correct amount to the attacker (recipient bind, lib.rs:743)");
}

// SELF-SERVICE FREEZE + RESIDUAL CLAIM (IX_FREEZE/IX_CLAIM, replacing the cranker seal). The decider
// holds its OWN funded COIN vault; after emission_end a permissionless freeze snapshots the cohort
// denominators, binds+verifies the vault (rd-owned, full supply, mint+freeze authority revoked) and
// closes accrual; then each backer pulls their OWN deterministic share to their bound recipient. Pins:
// freeze emission gate / snapshot / double-freeze / accrual-closed, and claim correctness +
// double-claim + decoy-vault + claim-before-freeze. No one-tx seal (IG dissolved); residual points are
// frozen-cumulative so there is no live/HE concern.
#[test]
fn self_service_freeze_and_residual_claim() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 900_000u64;
    let emission_end = 2_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    // The decider's OWN COIN vault: a token account owned by rd_config, funded with the whole supply.
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &rd_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);
    // rd init binds the canonical (uninitialized) distribution_config key; the claim path needs no
    // distribution program (the decider holds the vault and pays directly).
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&emission_end.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    // Two residual backers: alice (loss 1000) and bob (loss 500), each with 1e6 fees, hold 1000.
    let alice = Keypair::new(); let bob = Keypair::new();
    let a_ledger = Pubkey::new_unique(); let b_ledger = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 0, 0);
    set_backing_ledger(&mut svm, &b_ledger, &stub_perc, &bob.pubkey(), 0, 0);
    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    let b_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), bob.pubkey().as_ref()], &rd_id()).0;
    for (owner, ledger, stake) in [(&alice, &a_ledger, &a_stake), (&bob, &b_ledger, &b_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*ledger, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false)], data: vec![1u8, 0u8] }], &[owner]).expect("register");
    }
    set_slot(&mut svm, 1_100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 1_000, 1_000_000);
    set_backing_ledger(&mut svm, &b_ledger, &stub_perc, &bob.pubkey(), 500, 1_000_000);
    for (ledger, stake) in [(&a_ledger, &a_stake), (&b_ledger, &b_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*ledger, false)], data: vec![2u8] }], &[]).expect("crystallize");
    }
    // alice 9000, bob 4500, total 13500.
    assert_eq!(u128::from_le_bytes(svm.get_account(&rd_config).unwrap().data[154..170].try_into().unwrap()), 13_500);

    let freeze = |vault: &Pubkey| Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(*vault, false)], data: vec![4u8] };

    // (1) claim BEFORE freeze is rejected (denominators not final).
    let a_coin = create_token_account(&mut svm, &payer, &coin_mint, &alice.pubkey());
    let claim_a = Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(a_stake, false),
        AccountMeta::new(vault, false), AccountMeta::new(a_coin, false), AccountMeta::new_readonly(spl_token::ID, false)], data: vec![5u8] };
    assert!(send(&mut svm, &payer, &[claim_a.clone()], &[]).is_err(), "claim before freeze must reject");

    // (2) freeze BEFORE emission_end rejects; AFTER snapshots + binds the vault.
    set_slot(&mut svm, 1_500);
    assert!(send(&mut svm, &payer, &[freeze(&vault)], &[]).is_err(), "freeze before emission_end must reject");
    set_slot(&mut svm, 2_001);
    send(&mut svm, &payer, &[freeze(&vault)], &[]).expect("freeze");
    let cfg = svm.get_account(&rd_config).unwrap();
    assert_eq!(u128::from_le_bytes(cfg.data[286..302].try_into().unwrap()), 13_500, "frozen_total_points == total_points");
    assert_eq!(&cfg.data[326..358], vault.as_ref(), "vault bound");
    // (3) double-freeze rejects.
    assert!(send(&mut svm, &payer, &[freeze(&vault)], &[]).is_err(), "double-freeze must reject");

    // (4) DECOY vault: a different rd-owned coin account is rejected (only the bound vault pays).
    let decoy = create_token_account(&mut svm, &payer, &coin_mint, &rd_config);
    let claim_decoy = Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(a_stake, false),
        AccountMeta::new(decoy, false), AccountMeta::new(a_coin, false), AccountMeta::new_readonly(spl_token::ID, false)], data: vec![5u8] };
    assert!(send(&mut svm, &payer, &[claim_decoy], &[]).is_err(), "claim from a decoy vault must reject");

    // (4b) REDIRECT: a cranker tries to pull alice's share into the ATTACKER's own COIN account.
    // Blocked because the recipient_ata must be owned by the stake's BOUND recipient (finding GY,
    // re-checked at claim). This is the self-service analog of the seal-path anti-redirect (JA).
    let attacker = Keypair::new();
    let evil_ata = create_token_account(&mut svm, &payer, &coin_mint, &attacker.pubkey());
    let claim_redirect = Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(a_stake, false),
        AccountMeta::new(vault, false), AccountMeta::new(evil_ata, false), AccountMeta::new_readonly(spl_token::ID, false)], data: vec![5u8] };
    assert!(send(&mut svm, &payer, &[claim_redirect], &[]).is_err(), "claim must reject paying alice's share into the attacker's account");
    assert_eq!(token_amount(&svm, &evil_ata), 0, "attacker received no COIN");

    // (5) HAPPY: alice and bob each pull their deterministic share to their own COIN account.
    send(&mut svm, &payer, &[claim_a.clone()], &[]).expect("alice claim");
    assert_eq!(token_amount(&svm, &a_coin), 600_000, "alice's 9000/13500 of 900k");
    let b_coin = create_token_account(&mut svm, &payer, &coin_mint, &bob.pubkey());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(b_stake, false),
        AccountMeta::new(vault, false), AccountMeta::new(b_coin, false), AccountMeta::new_readonly(spl_token::ID, false)], data: vec![5u8] }], &[]).expect("bob claim");
    assert_eq!(token_amount(&svm, &b_coin), 300_000, "bob's 4500/13500 of 900k");

    // (6) DOUBLE-CLAIM: alice claims again -> rejected.
    assert!(send(&mut svm, &payer, &[claim_a], &[]).is_err(), "double-claim must reject");
    assert_eq!(token_amount(&svm, &a_coin), 600_000, "no extra COIN from a rejected double-claim");
}

// EZ (freeze-side): freeze REFUSES to bind a vault that does not hold the WHOLE supply. Claims pay
// floor(supply * points / denom) and sum to at most `supply`, so the vault MUST hold `supply` or the last
// claimers hit an empty vault (insolvency / LOF). The check is `v.amount < total_supply` (lib.rs:951).
// This isolates EZ from GX: the mint supply stays == total_supply (so the GX supply check passes), and only
// the vault BALANCE varies by one token — supply-1 rejects, supply accepts. Every freeze test funds the
// vault fully, so the underfunded reject side was unpinned.
#[test]
fn freeze_rejects_an_underfunded_vault() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 900_000u64;
    let emission_end = 2_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &rd_config);
    let decoy = create_token_account(&mut svm, &payer, &coin_mint, &payer.pubkey());
    // Mint the WHOLE supply (so GX's mint.supply == total_supply holds), but split it: supply-1 into the
    // vault, 1 into a decoy account the vault does NOT control. Then revoke the mint authority.
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &vault, supply - 1);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &decoy, 1);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&emission_end.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let freeze = Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(vault, false)], data: vec![4u8] };
    set_slot(&mut svm, 2_001);

    // (1) vault holds supply-1 -> freeze REJECTS (EZ). GX passes (mint.supply == supply), so this is EZ.
    assert!(send(&mut svm, &payer, &[freeze.clone()], &[]).is_err(), "freeze must reject a vault holding less than the whole supply");
    assert_eq!(svm.get_account(&rd_config).unwrap().data[318..326], [0u8; 8], "freeze_slot still 0 — not frozen");

    // (2) top the vault up to the full supply (a plain transfer, no mint authority needed) -> freeze succeeds.
    send(&mut svm, &payer, &[spl_token::instruction::transfer(&spl_token::ID, &decoy, &vault, &payer.pubkey(), &[], 1).unwrap()], &[]).expect("top up vault to full supply");
    assert_eq!(token_amount(&svm, &vault), supply, "vault now holds the whole supply");
    send(&mut svm, &payer, &[freeze], &[]).expect("freeze succeeds once the vault holds the whole supply");
    assert_ne!(svm.get_account(&rd_config).unwrap().data[318..326], [0u8; 8], "freeze_slot stamped");
}

// GX (freeze-side): freeze REFUSES to enter the claim phase while the COIN mint still carries a live
// mint OR freeze authority. This is fail-closed DOS prevention with teeth: a live FREEZE authority can
// freeze the bound vault (bricking EVERY claim) or selectively freeze a claimer's ATA; a live MINT
// authority can inflate supply out from under the frozen denominators. Every other test revokes the
// mint authority and uses a freeze_authority==None mint, so the POSITIVE freeze path never proves the
// rejection actually fires — this pins both GX sub-checks in isolation (mint-auth live -> reject;
// then mint-auth cleared but freeze-auth live -> still reject; then both cleared -> freeze succeeds),
// and demonstrates the freeze authority's real DOS power on a vault.
#[test]
fn freeze_rejects_a_mint_with_a_live_mint_or_freeze_authority() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 900_000u64;
    let emission_end = 2_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let freeze_auth = Keypair::new();
    // A COIN mint created WITH both a mint authority AND a freeze authority (the dangerous, un-revoked
    // state the orchestrator must clear before handing the supply to the decider).
    let mint = Keypair::new();
    let rent = svm.minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN);
    send(&mut svm, &payer, &[
        system_instruction::create_account(&payer.pubkey(), &mint.pubkey(), rent, spl_token::state::Mint::LEN as u64, &spl_token::ID),
        spl_token::instruction::initialize_mint(&spl_token::ID, &mint.pubkey(), &mint_auth.pubkey(), Some(&freeze_auth.pubkey()), 6).unwrap(),
    ], &[&mint]).expect("create mint with mint+freeze authority");
    let coin_mint = mint.pubkey();

    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &rd_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &vault, supply);

    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&emission_end.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    // One residual backer so there is a non-zero denominator to freeze.
    let alice = Keypair::new();
    let a_ledger = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 0, 0);
    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(alice.pubkey(), true),
        AccountMeta::new_readonly(alice.pubkey(), false), AccountMeta::new_readonly(a_ledger, false), AccountMeta::new(a_stake, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false)], data: vec![1u8, 0u8] }], &[&alice]).expect("register");
    set_slot(&mut svm, 1_100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 1_000, 1_000_000);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(a_stake, false), AccountMeta::new_readonly(a_ledger, false)], data: vec![2u8] }], &[]).expect("crystallize");

    let freeze = Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(vault, false)], data: vec![4u8] };
    set_slot(&mut svm, 2_001); // past emission_end + finalize_window(0)

    // (1) MINT authority still live -> freeze rejects (supply can be inflated under the denominators).
    assert!(send(&mut svm, &payer, &[freeze.clone()], &[]).is_err(), "freeze must reject a mint with a live mint authority");
    assert_eq!(svm.get_account(&rd_config).unwrap().data[318..326], [0u8; 8], "freeze_slot still 0 — not frozen");

    // (2) Revoke MINT authority but leave the FREEZE authority live -> still rejects.
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);
    assert!(send(&mut svm, &payer, &[freeze.clone()], &[]).is_err(), "freeze must reject a mint with a live freeze authority");

    // Demonstrate the freeze authority's real DOS power: it CAN freeze the bound vault, which would
    // brick every claim — exactly what GX exists to prevent the decider from committing to.
    send(&mut svm, &payer, &[spl_token::instruction::freeze_account(&spl_token::ID, &vault, &coin_mint, &freeze_auth.pubkey(), &[]).unwrap()], &[&freeze_auth]).expect("freeze authority can freeze the vault");
    send(&mut svm, &payer, &[spl_token::instruction::thaw_account(&spl_token::ID, &vault, &coin_mint, &freeze_auth.pubkey(), &[]).unwrap()], &[&freeze_auth]).expect("thaw back for the happy path");

    // (3) Clear the FREEZE authority too -> freeze now succeeds (both GX sub-checks satisfied).
    send(&mut svm, &payer, &[spl_token::instruction::set_authority(&spl_token::ID, &coin_mint, None, AuthorityType::FreezeAccount, &freeze_auth.pubkey(), &[]).unwrap()], &[&freeze_auth]).expect("clear freeze authority");
    send(&mut svm, &payer, &[freeze], &[]).expect("freeze succeeds once mint+freeze authorities are both cleared");
    assert_ne!(svm.get_account(&rd_config).unwrap().data[318..326], [0u8; 8], "freeze_slot stamped");
}

// SELF-SERVICE INSURANCE CLAIM with the ATOMIC HE CAP. The insurance cohort is a LIVE level (capital
// can be withdrawn), so its claim reads the position NOW and pays min(crystallized, live) in ONE tx —
// a depositor who partial-withdraws AFTER freeze claims only what the live principal justifies, never
// their stale-high crystallized points. Two equal depositors crystallize at 9000 pts each (total
// 18000); bob then withdraws 9/10 of his capital post-freeze. alice claims her full half (500k); bob
// is capped to his live tenth (~55.5k), NOT the 500k his stale points would pay.
#[test]
fn self_service_insurance_claim_caps_by_live_position() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64; // insurance_bps 10000 -> insurance_supply == supply.
    let emission_end = 2_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &rd_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&emission_end.to_le_bytes()); d.extend_from_slice(&10_000u16.to_le_bytes()); d.extend_from_slice(genesis_pool.as_ref());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let alice = Keypair::new(); let bob = Keypair::new();
    let a_pos = Pubkey::new_unique(); let b_pos = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_position(&mut svm, &a_pos, &stub_sub, &genesis_pool, &alice.pubkey(), 1_000, 100, false);
    set_position(&mut svm, &b_pos, &stub_sub, &genesis_pool, &bob.pubkey(), 1_000, 100, false);
    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    let b_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), bob.pubkey().as_ref()], &rd_id()).0;
    for (owner, pos, stake) in [(&alice, &a_pos, &a_stake), (&bob, &b_pos, &b_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*pos, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false)], data: vec![1u8, 1u8] }], &[owner]).expect("register insurance");
    }
    set_slot(&mut svm, 1_100); // hold 1000 -> floor_log2=9 -> 1000*9 = 9000 pts each.
    for (pos, stake) in [(&a_pos, &a_stake), (&b_pos, &b_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*pos, false)], data: vec![2u8] }], &[]).expect("crystallize");
    }
    assert_eq!(u128::from_le_bytes(svm.get_account(&rd_config).unwrap().data[174..190].try_into().unwrap()), 18_000, "insurance_total_points");

    set_slot(&mut svm, 2_001);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(vault, false)], data: vec![4u8] }], &[]).expect("freeze");

    // BOB withdraws 9/10 of his capital AFTER freeze: live principal 1000 -> 100.
    set_position(&mut svm, &b_pos, &stub_sub, &genesis_pool, &bob.pubkey(), 100, 100, false);

    // claim ix builder (insurance appends the position account).
    let claim = |stake: &Pubkey, pos: &Pubkey, coin: &Pubkey| Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(*stake, false),
        AccountMeta::new(vault, false), AccountMeta::new(*coin, false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(*pos, false)], data: vec![5u8] };

    let a_coin = create_token_account(&mut svm, &payer, &coin_mint, &alice.pubkey());
    send(&mut svm, &payer, &[claim(&a_stake, &a_pos, &a_coin)], &[]).expect("alice claim");
    assert_eq!(token_amount(&svm, &a_coin), 500_000, "alice (unchanged) gets her full 9000/18000 half");

    let b_coin = create_token_account(&mut svm, &payer, &coin_mint, &bob.pubkey());

    // SUBSTITUTED POSITION (HE-cap bypass): claim bob's stake but pass ALICE's un-withdrawn position
    // (principal 1000). Without the position bind bob's live_pts would be 10000 -> cap min(9000,10000)
    // = 9000 -> he'd over-claim his stale 500k despite having withdrawn 9/10. The position.key ==
    // stake.backing_ledger bind (lib.rs) rejects it. (The min-cap does NOT save this: a high-principal
    // position raises live_pts ABOVE stake.points, so the bind is the SOLE guard.)
    assert!(send(&mut svm, &payer, &[claim(&b_stake, &a_pos, &b_coin)], &[]).is_err(),
        "claiming bob's stake against alice's position must reject (HE cap depends on the bound position)");

    send(&mut svm, &payer, &[claim(&b_stake, &b_pos, &b_coin)], &[]).expect("bob claim");
    // bob capped: live_pts = 100 * floor_log2(2001-100=1901)=10 = 1000; min(9000,1000)=1000.
    // 1_000_000 * 1000 / 18000 = 55_555. NOT the 500_000 his stale 9000 points would pay.
    assert_eq!(token_amount(&svm, &b_coin), 55_555, "bob is HE-capped to his live tenth, not his stale half");
}

// POST-FREEZE TENURE RESET -> SATURATING FORFEIT (not an underflow over-claim). The insurance claim
// measures tenure as `freeze_slot - start_slot`. A position's start_slot is LAST-WRITE (the subledger
// resets it on every deposit, subledger:1082), so a depositor who tops up AFTER freeze ends up with
// start_slot > freeze_slot. insurance_points uses saturating_sub, so that tenure floors to 0 and the
// stake claims 0 (a self-inflicted forfeit). The load-bearing part: with a NON-saturating sub +
// overflow-checks (this repo's release profile), start>freeze would PANIC -> a per-stake claim DOS; and
// without overflow-checks it would wrap to ~u64::MAX -> floor_log2 ~63 -> a MASSIVE over-claim that drains
// the insurance supply. (A third party cannot inflict this: insurance_deposit requires owner.is_signer,
// subledger:925.) Every other insurance test has start < freeze; this pins the start > freeze edge.
#[test]
fn insurance_claim_with_start_after_freeze_saturates_to_zero_no_overclaim() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;
    let emission_end = 2_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &rd_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&emission_end.to_le_bytes()); d.extend_from_slice(&10_000u16.to_le_bytes()); d.extend_from_slice(genesis_pool.as_ref());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    // alice: sole insurance depositor, joins at slot 100.
    let alice = Keypair::new();
    let a_pos = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_position(&mut svm, &a_pos, &stub_sub, &genesis_pool, &alice.pubkey(), 1_000, 100, false);
    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(alice.pubkey(), true),
        AccountMeta::new_readonly(alice.pubkey(), false), AccountMeta::new_readonly(a_pos, false), AccountMeta::new(a_stake, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false)], data: vec![1u8, 1u8] }], &[&alice]).expect("register insurance");
    set_slot(&mut svm, 1_100); // crystallize with a real tenure: 1000 * floor_log2(1000)=9 = 9000 pts.
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(a_stake, false), AccountMeta::new_readonly(a_pos, false)], data: vec![2u8] }], &[]).expect("crystallize");
    assert_eq!(u128::from_le_bytes(svm.get_account(&rd_config).unwrap().data[174..190].try_into().unwrap()), 9_000, "denom");

    set_slot(&mut svm, 2_001);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(vault, false)], data: vec![4u8] }], &[]).expect("freeze");

    // POST-FREEZE re-deposit: last-write resets start_slot to AFTER freeze_slot (2001 < 5000).
    set_position(&mut svm, &a_pos, &stub_sub, &genesis_pool, &alice.pubkey(), 1_000, 5_000, false);

    // claim: live tenure = freeze_slot(2001) - start(5000) saturates to 0 -> live_pts 0 -> pays 0.
    let a_coin = create_token_account(&mut svm, &payer, &coin_mint, &alice.pubkey());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(a_stake, false),
        AccountMeta::new(vault, false), AccountMeta::new(a_coin, false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(a_pos, false)], data: vec![5u8] }], &[]).expect("claim must SUCCEED (saturate), not panic");
    assert_eq!(token_amount(&svm, &a_coin), 0, "start>freeze saturates to 0 tenure -> 0 COIN (self-forfeit), NOT an underflow over-claim");
    assert_eq!(token_amount(&svm, &vault), supply, "vault fully intact — nothing over-drawn");
    assert_eq!(svm.get_account(&a_stake).unwrap().data[210], 1, "stake consumed (claimed)");
}

// CROSS-CONFIG (cross-genesis) CLAIM SUBSTITUTION. Two independent genesis instances A and B each have
// their own rd_config + COIN vault. An attacker farms a HIGH-points stake in B (a cheap/throwaway
// genesis), then claims the REAL genesis A's vault while passing B's high-points stake — points/A.denom
// would over-draw A's vault if the stake weren't bound to A. The claim binds `stake.config ==
// config_account.key` (lib.rs ~ the freeze/vault checks). This test isolates that bind: config A is fully
// set up + frozen; B only needs an existing high-points stake (no vault/freeze). The claim passes A's
// config + A's vault + a valid A-recipient ATA, so every check up to the stake.config bind passes — the
// rejection is precisely that bind. Without it, B's points would drain A.
#[test]
fn claim_rejects_a_stake_from_a_foreign_config() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let alice = Keypair::new();

    // Helper: init an rd_config for `coin_mint` (residual-only), returns (rd_config, dist_config).
    let init_cfg = |svm: &mut LiteSVM, coin_mint: &Pubkey, supply: u64, emission_end: u64| -> Pubkey {
        let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
        let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
        let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
        d.extend_from_slice(&emission_end.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
        send(svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(*coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
            AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
            AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: d }], &[]).expect("rd init");
        rd_config
    };
    let reg_cryst = |svm: &mut LiteSVM, rd_config: &Pubkey, ledger: &Pubkey, stake: &Pubkey, loss: u128| {
        send(svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(*rd_config, false), AccountMeta::new_readonly(alice.pubkey(), true),
            AccountMeta::new_readonly(alice.pubkey(), false), AccountMeta::new_readonly(*ledger, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false)], data: vec![1u8, 0u8] }], &[&alice]).expect("register");
        set_slot(svm, 1_100);
        set_backing_ledger(svm, ledger, &stub_perc, &alice.pubkey(), loss, 1_000_000);
        send(svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(*rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*ledger, false)], data: vec![2u8] }], &[]).expect("crystallize");
    };

    // Genesis A: the REAL one — funded vault, small stake, frozen.
    let supply_a = 900_000u64;
    let mint_auth = Keypair::new();
    let coin_a = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_a = Pubkey::find_program_address(&[b"rd_config", coin_a.as_ref()], &rd_id()).0;
    let vault_a = create_token_account(&mut svm, &payer, &coin_a, &rd_a);
    mint_to(&mut svm, &payer, &coin_a, &mint_auth, &vault_a, supply_a);
    revoke_mint(&mut svm, &payer, &coin_a, &mint_auth);
    init_cfg(&mut svm, &coin_a, supply_a, 2_000);
    let a_ledger = Pubkey::new_unique();
    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_a.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 0, 0);
    reg_cryst(&mut svm, &rd_a, &a_ledger, &a_stake, 1_000); // small loss
    set_slot(&mut svm, 2_001);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_a, false), AccountMeta::new_readonly(coin_a, false), AccountMeta::new_readonly(vault_a, false)], data: vec![4u8] }], &[]).expect("freeze A");

    // Genesis B: throwaway — only needs a HIGH-points stake (no vault, no freeze).
    let mint_auth_b = Keypair::new();
    let coin_b = create_mint(&mut svm, &payer, &mint_auth_b.pubkey());
    let rd_b = init_cfg(&mut svm, &coin_b, 1_000_000_000, 2_000);
    let b_ledger = Pubkey::new_unique();
    let b_stake = Pubkey::find_program_address(&[b"rd_stake", rd_b.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &b_ledger, &stub_perc, &alice.pubkey(), 0, 0);
    reg_cryst(&mut svm, &rd_b, &b_ledger, &b_stake, 1_000_000_000); // HUGE loss -> huge points

    // ATTACK: claim genesis A's vault using genesis B's high-points stake.
    let a_coin = create_token_account(&mut svm, &payer, &coin_a, &alice.pubkey());
    let evil = Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_a, false), AccountMeta::new(b_stake, false),
        AccountMeta::new(vault_a, false), AccountMeta::new(a_coin, false), AccountMeta::new_readonly(spl_token::ID, false)], data: vec![5u8] };
    assert!(send(&mut svm, &payer, &[evil], &[]).is_err(), "claiming A's vault with a foreign-config (B) stake must reject");
    assert_eq!(token_amount(&svm, &vault_a), supply_a, "A's vault is fully intact — no cross-genesis drain");
    assert_eq!(svm.get_account(&b_stake).unwrap().data[210], 0, "B's stake was not consumed");
}

// VAULT DELEGATE RUG -- VERIFIED IMPOSSIBLE AT THE SOURCE. Hypothesis: a setup party `approve`s itself
// as delegate for the whole supply, hands the vault to the rd_config PDA, then drains it out from under
// the claimers. Disproved: SPL's set_authority(AccountOwner) CLEARS delegate + delegated_amount +
// close_authority, and rd_config (a PDA with no approve instruction) can never re-set them -- so a vault
// handed to rd_config is solely rd-controlled and freeze needs no delegate check. This pins that SPL
// invariant (a future change would surface here) so the omitted check stays justified.
#[test]
fn set_authority_clears_delegate_no_vault_rug() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 900_000u64;
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let stub_perc = Pubkey::new_unique(); let stub_sub = Pubkey::new_unique();
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    // Setup party funds a vault it owns, approves ITSELF (attacker) as delegate, then hands the vault
    // to rd_config. The delegate persists across the owner change.
    let orchestrator = Keypair::new(); svm.airdrop(&orchestrator.pubkey(), 1_000_000_000).unwrap();
    let attacker = Keypair::new(); svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &orchestrator.pubkey());
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &vault, supply);
    send(&mut svm, &payer, &[spl_token::instruction::approve(&spl_token::ID, &vault, &attacker.pubkey(), &orchestrator.pubkey(), &[], supply).unwrap()], &[&orchestrator]).expect("approve delegate");
    send(&mut svm, &payer, &[spl_token::instruction::set_authority(&spl_token::ID, &vault, Some(&rd_config), AuthorityType::AccountOwner, &orchestrator.pubkey(), &[]).unwrap()], &[&orchestrator]).expect("hand vault to rd_config");
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    // EMPIRICAL: the delegate did NOT survive the AccountOwner change (SPL clears it), so it cannot
    // drain the PDA-owned vault -> the rug is impossible at the source, no freeze check needed.
    let sink = create_token_account(&mut svm, &payer, &coin_mint, &attacker.pubkey());
    assert!(send(&mut svm, &payer, &[spl_token::instruction::transfer(&spl_token::ID, &vault, &sink, &attacker.pubkey(), &[], supply).unwrap()], &[&attacker]).is_err(),
        "delegate cleared by set_authority(AccountOwner) -> cannot drain");
    assert_eq!(token_amount(&svm, &sink), 0, "attacker got nothing");
}

// PREMATURE-FREEZE GRIEF. freeze is permissionless; with a 0 window anyone could freeze the instant
// emission ends and forfeit a slower backer's un-crystallized points. The finalize_window gates freeze
// until emission_end + window, giving backers a guaranteed post-emission crystallize window (the user's
// "1 week to finalize your points"). Here: a backer is registered but NOT yet crystallized at
// emission_end; an early freeze is rejected; the backer crystallizes during the window; freeze after
// the window then captures their points.
#[test]
fn finalize_window_blocks_premature_freeze_and_preserves_late_crystallize() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 900_000u64;
    let emission_end = 2_000u64;
    let finalize_window = 1_000u64; // freeze allowed only at >= 3000.
    let stub_perc = Pubkey::new_unique(); let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &rd_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    // wire: supply, fee_bps, emission_end, insurance_bps=0, then (no pool/market_group), finalize_window.
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&emission_end.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes()); d.extend_from_slice(&finalize_window.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init with finalize_window");

    let alice = Keypair::new(); let a_ledger = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 0, 0);
    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(alice.pubkey(), true),
        AccountMeta::new_readonly(alice.pubkey(), false), AccountMeta::new_readonly(a_ledger, false), AccountMeta::new(a_stake, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false)], data: vec![1u8, 0u8] }], &[&alice]).expect("register");
    // loss accrues; alice has NOT crystallized yet.
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 1_000, 1_000_000);

    let freeze = Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(vault, false)], data: vec![4u8] };
    let crystallize = Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(a_stake, false), AccountMeta::new_readonly(a_ledger, false)], data: vec![2u8] };

    // (1) PREMATURE freeze right after emission_end (slot 2001 < 3000) is rejected -> alice not forfeited.
    set_slot(&mut svm, 2_001);
    assert!(send(&mut svm, &payer, &[freeze.clone()], &[]).is_err(), "freeze before emission_end+window must reject");

    // (2) alice crystallizes DURING the window (still open).
    set_slot(&mut svm, 2_500);
    send(&mut svm, &payer, &[crystallize], &[]).expect("crystallize during the finalize window");
    let pts = u128::from_le_bytes(svm.get_account(&rd_config).unwrap().data[154..170].try_into().unwrap());
    assert!(pts > 0, "alice's points accrued during the window");

    // (3) freeze AFTER the window (slot 3001) succeeds and captures alice's points.
    set_slot(&mut svm, 3_001);
    send(&mut svm, &payer, &[freeze], &[]).expect("freeze after emission_end+window");
    assert_eq!(u128::from_le_bytes(svm.get_account(&rd_config).unwrap().data[286..302].try_into().unwrap()), pts,
        "frozen denominator captured the late crystallize -> alice not griefed");
}

// ACCRUAL CLOSED AFTER FREEZE (post-freeze point injection -> vault over-draw). freeze snapshots the
// cohort denominators; claims pay points/frozen_denominator. If register (lib.rs:546) or crystallize
// (lib.rs:630) could run AFTER freeze, an attacker could mint a NEW stake's points against the ALREADY-
// frozen denominator: e.g. alice freezes as the sole backer (frozen_total_points == P, she claims 100%),
// then bob registers+crystallizes P more -> bob would also claim P/P == 100% of residual_supply, drawing
// a SECOND full supply from a vault that holds only one -> over-draw / a claimer left unpaid (LOF/DOS).
// The finalize_window test exercises crystallize DURING the window then freeze, but NOTHING tested the
// reject side: register/crystallize AFTER freeze. This pins it, and proves alice alone already owns 100%
// (so any injected post-freeze stake necessarily over-draws).
#[test]
fn register_and_crystallize_are_rejected_after_freeze() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 900_000u64;
    let emission_end = 2_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &rd_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&emission_end.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let reg = |svm: &mut LiteSVM, owner: &Keypair, ledger: &Pubkey, stake: &Pubkey| -> Result<(), String> {
        send(svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*ledger, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false)], data: vec![1u8, 0u8] }], &[owner])
    };
    let cryst = |stake: &Pubkey, ledger: &Pubkey| Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*ledger, false)], data: vec![2u8] };

    // alice registers + crystallizes as the SOLE backer (loss 1000, fees 1e6, hold 1000).
    let alice = Keypair::new();
    let a_ledger = Pubkey::new_unique();
    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 0, 0);
    reg(&mut svm, &alice, &a_ledger, &a_stake).expect("alice register");
    set_slot(&mut svm, 1_100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 1_000, 1_000_000);
    send(&mut svm, &payer, &[cryst(&a_stake, &a_ledger)], &[]).expect("alice crystallize");
    let p = u128::from_le_bytes(svm.get_account(&rd_config).unwrap().data[154..170].try_into().unwrap());
    assert!(p > 0, "alice has points");

    // freeze after emission_end.
    set_slot(&mut svm, 2_001);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(vault, false)], data: vec![4u8] }], &[]).expect("freeze");
    assert_eq!(u128::from_le_bytes(svm.get_account(&rd_config).unwrap().data[286..302].try_into().unwrap()), p, "frozen_total_points == P");

    // (1) POST-FREEZE REGISTER (bob, a fresh stake) is REJECTED -> no new claimant against the frozen denom.
    let bob = Keypair::new();
    let b_ledger = Pubkey::new_unique();
    let b_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), bob.pubkey().as_ref()], &rd_id()).0;
    set_backing_ledger(&mut svm, &b_ledger, &stub_perc, &bob.pubkey(), 1_000, 1_000_000);
    assert!(reg(&mut svm, &bob, &b_ledger, &b_stake).is_err(), "register after freeze must reject (denominators frozen)");
    assert_eq!(svm.get_account(&b_stake).map(|a| a.data.len()).unwrap_or(0), 0, "bob's stake was never created");

    // (2) POST-FREEZE CRYSTALLIZE (alice again, e.g. after more loss accrues) is REJECTED -> can't inflate points.
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 100_000, 1_000_000);
    assert!(send(&mut svm, &payer, &[cryst(&a_stake, &a_ledger)], &[]).is_err(), "crystallize after freeze must reject (denominators final)");
    assert_eq!(u128::from_le_bytes(svm.get_account(&rd_config).unwrap().data[154..170].try_into().unwrap()), p, "total_points unchanged by the rejected crystallize");

    // (3) IMPACT: alice alone owns 100% of the residual supply (insurance_bps 0 -> residual_supply == supply).
    // Any post-freeze stake that HAD been admitted would have drawn a second full supply from this vault.
    let a_coin = create_token_account(&mut svm, &payer, &coin_mint, &alice.pubkey());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(a_stake, false),
        AccountMeta::new(vault, false), AccountMeta::new(a_coin, false), AccountMeta::new_readonly(spl_token::ID, false)], data: vec![5u8] }], &[]).expect("alice claim");
    assert_eq!(token_amount(&svm, &a_coin), supply, "alice claims the ENTIRE supply (100%) — no room for an injected post-freeze stake");
    assert_eq!(token_amount(&svm, &vault), 0, "vault fully drained by the sole legitimate claimant");
}

// FAKE TOKEN PROGRAM -> CLAIM-NULLIFICATION GRIEF (finding KE, VERIFIED BLOCKED). claim is PERMISSIONLESS
// and marks the stake claimed BEFORE the payout CPI, which it builds with the caller-supplied
// `token_program.key`. The HYPOTHESIS: an attacker cranks a victim's claim through a substituted program
// that no-ops, consuming the stake (claimed=true) while paying 0 COIN -> the victim's share is forfeited,
// and repeating over every stake nullifies the WHOLE distribution. VERDICT: BLOCKED — a non-SPL
// token_program is rejected by spl_token::instruction::transfer's internal check_program_account (the rd
// claim propagates it with `?`) BEFORE the substituted program is ever invoked, and an explicit
// token_program == spl_token::ID guard (matching distribution:619) backs it up. So the stake is NEVER
// consumed by the substitution attempt, and the rightful recipient keeps her claim. This pins the
// end-to-end property (zero prior coverage); it also guards against a future refactor to a hand-built
// transfer that would lose the implicit check.
#[test]
fn claim_rejects_a_fake_token_program_that_would_nullify_a_share() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 900_000u64;
    let emission_end = 2_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &rd_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&emission_end.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    // alice: sole residual backer (loss 1000, fees 1e6, hold 1000).
    let alice = Keypair::new();
    let a_ledger = Pubkey::new_unique();
    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 0, 0);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(alice.pubkey(), true),
        AccountMeta::new_readonly(alice.pubkey(), false), AccountMeta::new_readonly(a_ledger, false), AccountMeta::new(a_stake, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false)], data: vec![1u8, 0u8] }], &[&alice]).expect("register");
    set_slot(&mut svm, 1_100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 1_000, 1_000_000);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(a_stake, false), AccountMeta::new_readonly(a_ledger, false)], data: vec![2u8] }], &[]).expect("crystallize");
    set_slot(&mut svm, 2_001);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(vault, false)], data: vec![4u8] }], &[]).expect("freeze");

    let a_coin = create_token_account(&mut svm, &payer, &coin_mint, &alice.pubkey());

    // ATTACK: a third party (attacker, NOT alice) cranks alice's claim with a NON-SPL program as
    // token_program (here a bare pubkey), trying to consume her stake while paying nothing.
    let attacker = Keypair::new();
    svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    let fake_token_program = Pubkey::new_unique();
    let evil_claim = Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(attacker.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(a_stake, false),
        AccountMeta::new(vault, false), AccountMeta::new(a_coin, false), AccountMeta::new_readonly(fake_token_program, false)], data: vec![5u8] };
    assert!(send(&mut svm, &attacker, &[evil_claim], &[]).is_err(), "claim with a fake token_program must be rejected");

    // The victim's stake was NOT consumed (claimed flag @ STAKE_SIZE-1 == 210 still 0) and got no COIN.
    assert_eq!(svm.get_account(&a_stake).unwrap().data[210], 0, "alice's stake must NOT be marked claimed by the failed grief");
    assert_eq!(token_amount(&svm, &a_coin), 0, "no COIN moved by the failed grief");

    // And alice can STILL claim her full share through the REAL spl_token program.
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(a_stake, false),
        AccountMeta::new(vault, false), AccountMeta::new(a_coin, false), AccountMeta::new_readonly(spl_token::ID, false)], data: vec![5u8] }], &[]).expect("alice's legitimate claim");
    assert_eq!(token_amount(&svm, &a_coin), supply, "alice receives her full share — the grief did not forfeit it");
    assert_eq!(svm.get_account(&a_stake).unwrap().data[210], 1, "now legitimately claimed");
}

// SELF-SERVICE MULTI-BACKER ROUNDING: 3 equal residual backers over a supply not divisible by 3. Each
// floors to supply/3; the floor sum is < supply (dust stuck, burned); crucially the LAST claimer is
// never short as the vault drains (Σ floor <= supply <= vault). Prior claim tests used even divisions,
// so floor-rounding across sequential claims was unexercised.
#[test]
fn self_service_multi_backer_rounding_no_overdraw_or_short() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64; // not divisible by 3.
    let stub_perc = Pubkey::new_unique(); let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &rd_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let backers: Vec<Keypair> = (0..3).map(|_| Keypair::new()).collect();
    let ledgers: Vec<Pubkey> = (0..3).map(|_| Pubkey::new_unique()).collect();
    set_slot(&mut svm, 100);
    for (b, l) in backers.iter().zip(ledgers.iter()) { set_backing_ledger(&mut svm, l, &stub_perc, &b.pubkey(), 0, 0); }
    let stakes: Vec<Pubkey> = backers.iter().map(|b| Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), b.pubkey().as_ref()], &rd_id()).0).collect();
    for ((b, l), s) in backers.iter().zip(ledgers.iter()).zip(stakes.iter()) {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(b.pubkey(), true),
            AccountMeta::new_readonly(b.pubkey(), false), AccountMeta::new_readonly(*l, false), AccountMeta::new(*s, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false)], data: vec![1u8, 0u8] }], &[b]).expect("register");
    }
    set_slot(&mut svm, 1_100);
    for (b, l) in backers.iter().zip(ledgers.iter()) { set_backing_ledger(&mut svm, l, &stub_perc, &b.pubkey(), 1_000, 1_000_000); } // equal loss -> equal 9000 pts each.
    for (l, s) in ledgers.iter().zip(stakes.iter()) {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*s, false), AccountMeta::new_readonly(*l, false)], data: vec![2u8] }], &[]).expect("crystallize");
    }
    set_slot(&mut svm, 2_001);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(vault, false)], data: vec![4u8] }], &[]).expect("freeze");

    // each of the 3 equal backers claims floor(1_000_000 / 3) = 333_333; the 3rd is NOT short.
    for (b, s) in backers.iter().zip(stakes.iter()) {
        let coin = create_token_account(&mut svm, &payer, &coin_mint, &b.pubkey());
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(*s, false),
            AccountMeta::new(vault, false), AccountMeta::new(coin, false), AccountMeta::new_readonly(spl_token::ID, false)], data: vec![5u8] }], &[]).expect("claim");
        assert_eq!(token_amount(&svm, &coin), 333_333, "each equal backer floors to supply/3 (incl. the last)");
    }
    assert_eq!(token_amount(&svm, &vault), 1, "floor rounding leaves 1 atom of dust stuck (burned), no over-draw");
}

// SELF-SERVICE ZERO-AMOUNT CLAIM (untested branch): a residual backer whose backing was NEVER drawn
// (loss 0 -> eligible 0 -> 0 points) claims. The claim's `amount > 0` guard (lib.rs:1029) must mark the
// stake claimed and transfer NOTHING (no error/panic), and the 0-point stake must not break the
// denominator for a real co-backer. Pins the amount==0 path + that a no-risk backer correctly gets 0.
#[test]
fn self_service_zero_amount_claim_pays_nothing_and_marks_claimed() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 900_000u64;
    let stub_perc = Pubkey::new_unique(); let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &rd_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let alice = Keypair::new(); let zero = Keypair::new();
    let a_ledger = Pubkey::new_unique(); let z_ledger = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 0, 0);
    set_backing_ledger(&mut svm, &z_ledger, &stub_perc, &zero.pubkey(), 0, 0); // zero stays loss-free forever.
    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    let z_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), zero.pubkey().as_ref()], &rd_id()).0;
    for (owner, ledger, stake) in [(&alice, &a_ledger, &a_stake), (&zero, &z_ledger, &z_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*ledger, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false)], data: vec![1u8, 0u8] }], &[owner]).expect("register");
    }
    set_slot(&mut svm, 1_100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 1_000, 1_000_000); // alice absorbed loss; zero did not.
    for (ledger, stake) in [(&a_ledger, &a_stake), (&z_ledger, &z_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*ledger, false)], data: vec![2u8] }], &[]).expect("crystallize");
    }
    // total_points = alice 9000 + zero 0 = 9000.
    assert_eq!(u128::from_le_bytes(svm.get_account(&rd_config).unwrap().data[154..170].try_into().unwrap()), 9_000);

    set_slot(&mut svm, 2_001);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(vault, false)], data: vec![4u8] }], &[]).expect("freeze");

    // zero (0 points) claims -> 0 COIN, no transfer, marked claimed (no error/panic).
    let z_coin = create_token_account(&mut svm, &payer, &coin_mint, &zero.pubkey());
    let z_claim = Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(z_stake, false),
        AccountMeta::new(vault, false), AccountMeta::new(z_coin, false), AccountMeta::new_readonly(spl_token::ID, false)], data: vec![5u8] };
    send(&mut svm, &payer, &[z_claim.clone()], &[]).expect("zero-point claim must succeed (no-op transfer)");
    assert_eq!(token_amount(&svm, &z_coin), 0, "no-loss backer is paid nothing");
    // and it is marked claimed (double-claim rejected) -> the amount==0 branch still consumes the stake.
    assert!(send(&mut svm, &payer, &[z_claim], &[]).is_err(), "zero-amount claim still marks the stake claimed");

    // the real co-backer is unaffected: alice claims the full residual_supply (the 0-point stake didn't dilute).
    let a_coin = create_token_account(&mut svm, &payer, &coin_mint, &alice.pubkey());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(a_stake, false),
        AccountMeta::new(vault, false), AccountMeta::new(a_coin, false), AccountMeta::new_readonly(spl_token::ID, false)], data: vec![5u8] }], &[]).expect("alice claim");
    assert_eq!(token_amount(&svm, &a_coin), 900_000, "the real backer gets the full supply; the 0-point stake didn't dilute the denominator");
}

// SELF-SERVICE CROSS-COHORT coexistence: a genesis with BOTH cohorts (insurance_bps=2000 -> 80% residual
// / 20% insurance) where a residual backer AND an insurance depositor each claim from the SAME vault.
// Verifies the supply split is exact and the two cohorts share the vault without over-draw (residual
// claims bounded by residual_supply, insurance by insurance_supply; sum == total_supply -> vault 0).
#[test]
fn self_service_cross_cohort_split_no_overdraw() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64; // insurance_bps 2000 -> insurance 200k, residual 800k.
    let emission_end = 2_000u64;
    let stub_perc = Pubkey::new_unique(); let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let vault = create_token_account(&mut svm, &payer, &coin_mint, &rd_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&emission_end.to_le_bytes()); d.extend_from_slice(&2_000u16.to_le_bytes()); d.extend_from_slice(genesis_pool.as_ref());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let alice = Keypair::new(); let bob = Keypair::new(); // alice = residual backer, bob = insurance depositor.
    let a_ledger = Pubkey::new_unique(); let b_pos = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 0, 0);
    set_position(&mut svm, &b_pos, &stub_sub, &genesis_pool, &bob.pubkey(), 1_000, 100, false);
    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    let b_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), bob.pubkey().as_ref()], &rd_id()).0;
    // register: alice residual (cohort 0), bob insurance (cohort 1).
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(alice.pubkey(), true),
        AccountMeta::new_readonly(alice.pubkey(), false), AccountMeta::new_readonly(a_ledger, false), AccountMeta::new(a_stake, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false)], data: vec![1u8, 0u8] }], &[&alice]).expect("alice register");
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(bob.pubkey(), true),
        AccountMeta::new_readonly(bob.pubkey(), false), AccountMeta::new_readonly(b_pos, false), AccountMeta::new(b_stake, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false)], data: vec![1u8, 1u8] }], &[&bob]).expect("bob register");
    set_slot(&mut svm, 1_100);
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &alice.pubkey(), 1_000, 1_000_000);
    for (linked, stake) in [(&a_ledger, &a_stake), (&b_pos, &b_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*linked, false)], data: vec![2u8] }], &[]).expect("crystallize");
    }

    set_slot(&mut svm, 2_001);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(vault, false)], data: vec![4u8] }], &[]).expect("freeze");

    // alice (sole residual) claims the full residual_supply (800k); bob (sole insurance) the full
    // insurance_supply (200k). Both from the SAME vault; sum == supply -> vault drains to 0.
    let a_coin = create_token_account(&mut svm, &payer, &coin_mint, &alice.pubkey());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(a_stake, false),
        AccountMeta::new(vault, false), AccountMeta::new(a_coin, false), AccountMeta::new_readonly(spl_token::ID, false)], data: vec![5u8] }], &[]).expect("alice residual claim");
    let b_coin = create_token_account(&mut svm, &payer, &coin_mint, &bob.pubkey());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(b_stake, false),
        AccountMeta::new(vault, false), AccountMeta::new(b_coin, false), AccountMeta::new_readonly(spl_token::ID, false), AccountMeta::new_readonly(b_pos, false)], data: vec![5u8] }], &[]).expect("bob insurance claim");
    assert_eq!(token_amount(&svm, &a_coin), 800_000, "residual backer gets the full 80% residual_supply");
    assert_eq!(token_amount(&svm, &b_coin), 200_000, "insurance depositor gets the full 20% insurance_supply");
    assert_eq!(token_amount(&svm, &vault), 0, "both cohorts shared the vault with no over-draw or dust");
}

// Mock subledger Position: principal@72, withdrawn@88, start_slot@89 (the offsets the decider reads).
fn set_position(svm: &mut LiteSVM, key: &Pubkey, sub: &Pubkey, pool: &Pubkey, owner: &Pubkey, principal: u64, start: u64, withdrawn: bool) {
    // FAITHFUL to the real subledger Position layout: disc@0, pool@8..40, owner@40..72,
    // principal@72, withdrawn@88, start_slot@89, shares@104.
    let mut data = vec![0u8; 120];
    data[8..40].copy_from_slice(pool.as_ref()); // Position.pool @ 8 (scoped by config.subledger_pool, HG)
    data[40..72].copy_from_slice(owner.as_ref()); // Position.owner @ 40
    data[72..80].copy_from_slice(&principal.to_le_bytes());
    data[88] = withdrawn as u8;
    data[89..97].copy_from_slice(&start.to_le_bytes());
    svm.set_account(*key, Account { lamports: 1_000_000_000, data, owner: *sub, executable: false, rent_epoch: 0 }).unwrap();
}

// INSURANCE COHORT (20%) + SOFT-VETO FORFEITURE. residual backer gets 80%; two insurance
// depositors split the 20% by capital*log-time; one EXITS and forfeits (its share is burned,
// not redistributed) and the decider REFUSES to seal a proposal that pays a withdrawn depositor.
#[test]
fn insurance_cohort_split_and_exit_forfeiture() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    svm.add_program_from_file(dist_id(), dist_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;          // insurance_bps 2000 -> insurance 200k, residual 800k.
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique(); // the one insurance pool the cohort is scoped to (HG)
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let dist_vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &dist_vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(dist_config, false),
        AccountMeta::new_readonly(dist_vault, false), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("dist init");
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&2_000u16.to_le_bytes()); // emission_end, insurance_bps=20%
    d.extend_from_slice(genesis_pool.as_ref()); // subledger_pool (HG)
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let backer = Keypair::new(); let alice = Keypair::new(); let bob = Keypair::new();
    let b_ledger = Pubkey::new_unique();
    let a_pos = Pubkey::new_unique(); let bob_pos = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &b_ledger, &stub_perc, &backer.pubkey(), 0, 0);
    set_position(&mut svm, &a_pos, &stub_sub, &genesis_pool, &alice.pubkey(), 100, 100, false);
    set_position(&mut svm, &bob_pos, &stub_sub, &genesis_pool, &bob.pubkey(), 100, 100, false);

    let s_backer = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), backer.pubkey().as_ref()], &rd_id()).0;
    let s_alice = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    let s_bob = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), bob.pubkey().as_ref()], &rd_id()).0;
    // register: backer residual (cohort 0), alice+bob insurance (cohort 1). owner signs (authorizes recipient).
    for (owner, linked, stake, cohort) in [(&backer,&b_ledger,&s_backer,0u8),(&alice,&a_pos,&s_alice,1u8),(&bob,&bob_pos,&s_bob,1u8)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*linked, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, cohort] }], &[owner]).expect("register");
    }
    // backer absorbs residual; crystallize all at slot 1100 (insurance hold=1000 -> log2=9 -> 900 pts each).
    set_backing_ledger(&mut svm, &b_ledger, &stub_perc, &backer.pubkey(), 1_000, 1_000_000);
    set_slot(&mut svm, 1_100);
    for (stake, linked) in [(&s_backer,&b_ledger),(&s_alice,&a_pos),(&s_bob,&bob_pos)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*linked, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }
    // BOB EXITS: his subledger position is now withdrawn -> his COIN is forfeited.
    set_position(&mut svm, &bob_pos, &stub_sub, &genesis_pool, &bob.pubkey(), 0, 100, true);

    set_slot(&mut svm, 2_001);
    let proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &1u64.to_le_bytes()], &dist_id()).0;
    let mut cp = vec![1u8]; cp.extend_from_slice(&1u64.to_le_bytes()); cp.extend_from_slice(&2u32.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: cp }], &[]).expect("create proposal");
    // Honest proposal: backer 800k (residual), alice 100k (her half of the 200k insurance; bob's
    // half stays in insurance_total -> burned). bob is EXCLUDED (forfeited).
    let mut ap = vec![2u8]; ap.extend_from_slice(&2u32.to_le_bytes());
    ap.extend_from_slice(backer.pubkey().as_ref()); ap.extend_from_slice(&800_000u64.to_le_bytes());
    ap.extend_from_slice(alice.pubkey().as_ref()); ap.extend_from_slice(&100_000u64.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false),
    ], data: ap }], &[]).expect("append");
    // seal: backer (residual, stake only), alice (insurance entry, stake + live position), then the
    // forfeited bob passed as a trailing (stake, position) EXTRA so insurance completeness (HX) is
    // satisfied -- bob's points stay in the denominator (his share burned) but every crystallized
    // insurance stake must be accounted for, exactly as the residual cohort requires (HD).
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new(dist_config, false), AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(s_backer, false), AccountMeta::new_readonly(s_alice, false), AccountMeta::new_readonly(a_pos, false),
        AccountMeta::new_readonly(s_bob, false), AccountMeta::new_readonly(bob_pos, false),
    ], data: vec![3u8] }], &[]).expect("decider seals 80/20 split");

    let b_ata = create_token_account(&mut svm, &payer, &coin_mint, &backer.pubkey());
    let a_ata = create_token_account(&mut svm, &payer, &coin_mint, &alice.pubkey());
    for (sig, idx, ata) in [(&backer, 0u32, &b_ata), (&alice, 1u32, &a_ata)] {
        let mut cl = vec![4u8]; cl.extend_from_slice(&idx.to_le_bytes());
        send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
            AccountMeta::new(sig.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false),
            AccountMeta::new(dist_vault, false), AccountMeta::new(*ata, false), AccountMeta::new_readonly(spl_token::ID, false),
        ], data: cl }], &[sig]).expect("claim");
    }
    assert_eq!(token_amount(&svm, &b_ata), 800_000, "residual backer gets 80%");
    assert_eq!(token_amount(&svm, &a_ata), 100_000, "alice gets her half of the 20% insurance cohort");
    // bob's 100k (his forfeited half of the insurance cohort) is unclaimed in the vault -> burnable.
    assert_eq!(token_amount(&svm, &dist_vault), 100_000, "the exited depositor's forfeited share remains unallocated (burned as unclaimed)");
}

// THEFT / FRONT-RUN (governance LOF, finding GY): register_start must bind a stake to the RIGHTFUL
// party of its linked account and require that party's SIGNATURE. The COIN reward for a backing
// ledger is owed to the ledger's `authority` (the LP who absorbed the loss); for an insurance
// position, to the position's `owner`. Without these checks a hostile actor could (a) point a stake
// at a VICTIM's backing ledger / position with recipient = attacker and harvest the COIN the
// victim's capital-at-risk earned, or (b) front-run the victim by creating the victim's own
// (per-owner) stake PDA with recipient = attacker — after which the victim could never register.
// Either drains the victim's rightful share of the fixed COIN supply.
#[test]
fn register_start_rejects_farming_or_front_running_an_unowned_stake() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    // rd init (distribution accounts are only stored; register_start needs config + percolator/subledger ids).
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&2_000u16.to_le_bytes());
    d.extend_from_slice(genesis_pool.as_ref()); // subledger_pool (HG)
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let victim = Keypair::new();
    let attacker = Keypair::new();
    let attacker2 = Keypair::new();
    svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    svm.airdrop(&attacker2.pubkey(), 1_000_000_000).unwrap();
    let v_ledger = Pubkey::new_unique();
    let v_pos = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &v_ledger, &stub_perc, &victim.pubkey(), 1_000, 1_000_000);
    set_position(&mut svm, &v_pos, &stub_sub, &genesis_pool, &victim.pubkey(), 100, 100, false);

    // register_start ix for (owner, recipient, linked, owner_signs, cohort).
    let reg = |owner: &Pubkey, recipient: &Pubkey, linked: &Pubkey, owner_signs: bool, cohort: u8| {
        let stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), owner.as_ref()], &rd_id()).0;
        Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false),
            AccountMeta::new_readonly(*owner, owner_signs), AccountMeta::new_readonly(*recipient, false),
            AccountMeta::new_readonly(*linked, false), AccountMeta::new(stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, cohort] }
    };

    // (a) FARM: attacker's own stake points at the victim's backing ledger, recipient = attacker.
    let r = send(&mut svm, &payer, &[reg(&attacker.pubkey(), &attacker.pubkey(), &v_ledger, true, 0)], &[&attacker]);
    assert!(r.is_err(), "attacker must NOT farm the victim's backing ledger (authority != owner)");

    // (b) FRONT-RUN: attacker creates the VICTIM's stake PDA (owner = victim) with recipient =
    //     attacker, WITHOUT the victim's signature.
    let r = send(&mut svm, &payer, &[reg(&victim.pubkey(), &attacker.pubkey(), &v_ledger, false, 0)], &[]);
    assert!(r.is_err(), "attacker must NOT front-run the victim's stake without the victim's signature");

    // (c) INSURANCE FARM: attacker points an insurance stake at the victim's subledger position.
    let r = send(&mut svm, &payer, &[reg(&attacker2.pubkey(), &attacker2.pubkey(), &v_pos, true, 1)], &[&attacker2]);
    assert!(r.is_err(), "attacker must NOT farm the victim's insurance position (position.owner != owner)");

    // LEGIT: the rightful LP registers their own stake, signing, against their own ledger.
    let r = send(&mut svm, &payer, &[reg(&victim.pubkey(), &victim.pubkey(), &v_ledger, true, 0)], &[&victim]);
    assert!(r.is_ok(), "the rightful LP can register their own stake: {:?}", r);
}

// PERMISSIONLESS-CRYSTALLIZE GRIEF (governance LOF/DOS, finding GZ): crystallize is permissionless
// (any cranker, no binding to the stake owner). Residual points must depend only on (total eligible
// residual, TRUE tenure) — not on how often / when crystallize is cranked. Under the per-window
// `eligible * floor_log2(hold)` scheme that reset start_slot each call, a hostile cranker could
// crystallize a victim's stake at a tiny hold right after the residual posts, capturing ALL the
// eligible into a floor_log2(1)=0 window and advancing the snapshot — so the victim can never re-earn
// it and their COIN share is griefed to ~0. The fix accumulates eligible and applies the tenure
// multiplier to the running total against the ORIGINAL start_slot, so a final crystallize recovers.
#[test]
fn permissionless_crystallize_cannot_grief_a_residual_stake_to_zero() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&5_000u64.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let victim = Keypair::new();
    let griefer = Keypair::new();
    svm.airdrop(&victim.pubkey(), 1_000_000_000).unwrap();
    svm.airdrop(&griefer.pubkey(), 1_000_000_000).unwrap();
    let v_ledger = Pubkey::new_unique();
    let v_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), victim.pubkey().as_ref()], &rd_id()).0;

    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &v_ledger, &stub_perc, &victim.pubkey(), 0, 0);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(victim.pubkey(), true),
        AccountMeta::new_readonly(victim.pubkey(), false), AccountMeta::new_readonly(v_ledger, false), AccountMeta::new(v_stake, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: vec![1u8, 0u8] }], &[&victim]).expect("register");

    // residual posts at slot 101 (1000 loss, 1e6 fee -> eligible = min(1000, 8000) = 1000).
    set_slot(&mut svm, 101);
    set_backing_ledger(&mut svm, &v_ledger, &stub_perc, &victim.pubkey(), 1_000, 1_000_000);

    let crystallize = |svm: &mut LiteSVM, signer: &Keypair| {
        let ix = Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(signer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(v_stake, false), AccountMeta::new_readonly(v_ledger, false),
        ], data: vec![2u8] };
        send(svm, signer, &[ix], &[])
    };
    // GRIEF: attacker crystallizes at slot 101 (hold = 1 -> floor_log2 = 0), consuming the snapshot.
    crystallize(&mut svm, &griefer).expect("griefer crystallize");
    // The victim cranks a final crystallize before seal (slot 1100, true tenure = 1000).
    set_slot(&mut svm, 1_100);
    crystallize(&mut svm, &victim).expect("victim crystallize");

    let pts = u128::from_le_bytes(svm.get_account(&v_stake).unwrap().data[176..192].try_into().unwrap());
    // 1000 eligible x floor_log2(1000)=9 = 9000, independent of the griefer's early window.
    assert_eq!(pts, 9_000, "victim's points must reflect total eligible x true-tenure log, not be griefed to 0");
}

// LATE-REGISTRATION DILUTION (governance LOF, finding HY): residual points must reward ONLY the
// residual loss a backer absorbed WHILE its stake was registered (capital actually at risk during the
// loss) — not loss the market had ALREADY accrued before the backer showed up. register_start snapshots
// the backing ledger's live `cumulative_loss` into `residual_snap`, and crystallize derives
// `total_res = cumulative_loss - residual_snap` (HO: the snap NEVER advances). So a hostile latecomer
// who provides backing to a market that has ALREADY taken a big residual loss, then registers to mint
// COIN points for that pre-existing loss, gets eligible = 0 -> points = 0, and cannot dilute the fixed
// COIN supply away from the honest backers who were genuinely at risk when the loss happened. Pinned
// as a CONTRAST against an honest backer whose loss accrues AFTER registration: identical ledger,
// identical tenure, differing ONLY in pre- vs post-registration timing of the loss -> 0 vs 9000 points.
// (Non-tautological: drop the residual_snap capture and the latecomer would mint 9000 too -> dilution.)
#[test]
fn register_after_loss_cannot_claim_pre_registration_residual() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&5_000u64.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let attacker = Keypair::new(); let honest = Keypair::new();
    svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    svm.airdrop(&honest.pubkey(), 1_000_000_000).unwrap();
    let a_ledger = Pubkey::new_unique(); let h_ledger = Pubkey::new_unique();
    let a_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), attacker.pubkey().as_ref()], &rd_id()).0;
    let h_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), honest.pubkey().as_ref()], &rd_id()).0;

    set_slot(&mut svm, 100);
    // ATTACKER's ledger has ALREADY absorbed 1000 residual loss (with fee support) BEFORE registration.
    set_backing_ledger(&mut svm, &a_ledger, &stub_perc, &attacker.pubkey(), 1_000, 1_000_000);
    // HONEST's ledger is pristine at registration (loss accrues AFTER, below).
    set_backing_ledger(&mut svm, &h_ledger, &stub_perc, &honest.pubkey(), 0, 0);
    for (owner, ledger, stake) in [(&attacker,&a_ledger,&a_stake),(&honest,&h_ledger,&h_stake)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*ledger, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 0u8] }], &[owner]).expect("register");
    }
    // The HONEST backer's loss accrues AFTER registration (same magnitude + fee support as the attacker's
    // pre-existing loss). The attacker's ledger is UNCHANGED (no new loss after they joined).
    set_backing_ledger(&mut svm, &h_ledger, &stub_perc, &honest.pubkey(), 1_000, 1_000_000);

    // Identical tenure window for both: crystallize at slot 1100 (hold = 1000 -> floor_log2 = 9).
    set_slot(&mut svm, 1_100);
    for (stake, ledger) in [(&a_stake,&a_ledger),(&h_stake,&h_ledger)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*ledger, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }

    let a_pts = u128::from_le_bytes(svm.get_account(&a_stake).unwrap().data[176..192].try_into().unwrap());
    let h_pts = u128::from_le_bytes(svm.get_account(&h_stake).unwrap().data[176..192].try_into().unwrap());
    // total_res = cumulative_loss - residual_snap. Attacker: 1000 - 1000 = 0 (loss predates the snap).
    assert_eq!(a_pts, 0, "a backer cannot mint COIN points for residual loss that accrued BEFORE it registered");
    // Honest: 1000 - 0 = 1000 eligible x floor_log2(1000)=9 -> 9000. Same ledger/tenure, post-reg loss.
    assert_eq!(h_pts, 9_000, "the honest backer earns for loss absorbed while registered (snapshot contrast)");
    // The fixed COIN supply is therefore undiluted by the latecomer (total_points reflects only honest).
    let total_points = u128::from_le_bytes(svm.get_account(&rd_config).unwrap().data[154..170].try_into().unwrap());
    assert_eq!(total_points, 9_000, "config total_points excludes the latecomer's pre-registration loss");
}

// FEE-MISALIGNMENT CRYSTALLIZE GRIEF (governance LOF/DOS, finding HO; extends GZ): the residual
// fee-cap eligible = min(Δresidual, Δfee*1e4/bps) was computed PER WINDOW with the snapshot advancing
// each crystallize. percolator's residual (cumulative_loss) and fees (total_earnings) rise via
// SEPARATE events, so a permissionless griefer can crystallize in a window where residual accrued but
// fees haven't synced (Δfee=0 -> eligible 0), consuming that residual for nothing — the later
// fee-window then sees Δresidual=0 and also yields 0. Points must instead derive from REGISTER-to-now
// TOTALS so the fee cap applies to the whole period regardless of crank timing.
#[test]
fn permissionless_crystallize_cannot_grief_via_fee_misalignment() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let supply = 1_000_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&5_000u64.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let victim = Keypair::new();
    let griefer = Keypair::new();
    svm.airdrop(&victim.pubkey(), 1_000_000_000).unwrap();
    svm.airdrop(&griefer.pubkey(), 1_000_000_000).unwrap();
    let v_ledger = Pubkey::new_unique();
    let v_stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), victim.pubkey().as_ref()], &rd_id()).0;

    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &v_ledger, &stub_perc, &victim.pubkey(), 0, 0);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(victim.pubkey(), true),
        AccountMeta::new_readonly(victim.pubkey(), false), AccountMeta::new_readonly(v_ledger, false), AccountMeta::new(v_stake, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: vec![1u8, 0u8] }], &[&victim]).expect("register");

    let crystallize = |svm: &mut LiteSVM, signer: &Keypair| {
        let ix = Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(signer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(v_stake, false), AccountMeta::new_readonly(v_ledger, false),
        ], data: vec![2u8] };
        send(svm, signer, &[ix], &[])
    };
    // Residual accrues but fees have NOT synced yet (loss=1000, earnings=0).
    set_slot(&mut svm, 200);
    set_backing_ledger(&mut svm, &v_ledger, &stub_perc, &victim.pubkey(), 1_000, 0);
    // GRIEF: attacker crystallizes in this fees-lagging window.
    crystallize(&mut svm, &griefer).expect("griefer crystallize (fees lagging)");
    // Fees sync later (loss unchanged at 1000, earnings=1e6).
    set_slot(&mut svm, 300);
    set_backing_ledger(&mut svm, &v_ledger, &stub_perc, &victim.pubkey(), 1_000, 1_000_000);
    // Victim cranks before seal.
    set_slot(&mut svm, 1_100);
    crystallize(&mut svm, &victim).expect("victim crystallize");

    let pts = u128::from_le_bytes(svm.get_account(&v_stake).unwrap().data[176..192].try_into().unwrap());
    // From register-to-now totals: eligible = min(1000, 1e6*1e4/80) = 1000; tenure 1000 -> log2=9 -> 9000.
    assert_eq!(pts, 9_000, "fee cap must apply to register-to-now totals, immune to a fees-lagging griefer window");
}

// CROSS-POOL INSURANCE FARMING (LOF, finding HG): the insurance cohort is scoped to ONE genesis
// pool (config.subledger_pool). The subledger program is shared across pools (assets 1..N), so
// without scoping, a depositor in ANY other pool could register that position for this genesis's
// insurance COIN, diluting the legit depositors. register_start must reject a position whose
// Position.pool != config.subledger_pool.
#[test]
fn register_insurance_rejects_a_position_from_a_foreign_pool() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique();
    let foreign_pool = Pubkey::new_unique();

    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&2_000u16.to_le_bytes());
    d.extend_from_slice(genesis_pool.as_ref());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let attacker = Keypair::new();
    svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    let foreign_pos = Pubkey::new_unique();
    let own_pos = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    // Both positions are owned by the attacker; one is in a FOREIGN pool, one in the genesis pool.
    set_position(&mut svm, &foreign_pos, &stub_sub, &foreign_pool, &attacker.pubkey(), 100, 100, false);
    set_position(&mut svm, &own_pos, &stub_sub, &genesis_pool, &attacker.pubkey(), 100, 100, false);

    let reg = |linked: &Pubkey| {
        let stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), attacker.pubkey().as_ref()], &rd_id()).0;
        Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false),
            AccountMeta::new_readonly(attacker.pubkey(), true), AccountMeta::new_readonly(attacker.pubkey(), false),
            AccountMeta::new_readonly(*linked, false), AccountMeta::new(stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 1u8] }
    };
    // Foreign-pool position must be rejected (scoping).
    let r = send(&mut svm, &payer, &[reg(&foreign_pos)], &[&attacker]);
    assert!(r.is_err(), "an insurance position from a foreign pool must be rejected (HG scoping)");
    // The attacker's genesis-pool position registers fine (positive control).
    let r = send(&mut svm, &payer, &[reg(&own_pos)], &[&attacker]);
    assert!(r.is_ok(), "a genesis-pool position registers normally: {:?}", r);
}

// TYPE-COSPLAY, INSURANCE SIDE (symmetric to register_rejects_a_residual_ledger_not_owned_by_the_
// percolator_program / IZ, which covers the residual side). An INSURANCE stake's tenure points are read
// from the linked account's subledger-Position bytes (principal@72, start@89). If an attacker could pass a
// non-subledger account shaped like a Position, they'd mint insurance points against bytes a different
// program authored. The defense is the OWNER check (lib.rs:571): an insurance `linked` MUST be owned by
// config.subledger_program. Here register is handed a PERFECTLY-shaped Position (pool@8 == genesis_pool so
// the HG scope passes, owner@40 == attacker so the GY bind passes) that is owned by the WRONG program
// (stub_perc, the percolator program — set_position's 3rd arg is the account owner). A subledger-owned but
// otherwise-identical position is the positive control, so the ONLY difference is the linked.owner ->
// lib.rs:571 is the SOLE blocker (a non-tautological isolation: drop :571 and the cosplay would register).
#[test]
fn register_insurance_rejects_a_linked_not_owned_by_the_subledger_program() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique();

    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&2_000u16.to_le_bytes());
    d.extend_from_slice(genesis_pool.as_ref());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let attacker = Keypair::new();
    svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    set_slot(&mut svm, 100);
    // COSPLAY: a Position-shaped account owned by the PERCOLATOR program (wrong type for the insurance
    // cohort), with pool@8 and owner@40 set to pass every check EXCEPT the owner-type check.
    let cosplay = Pubkey::new_unique();
    set_position(&mut svm, &cosplay, &stub_perc, &genesis_pool, &attacker.pubkey(), 100, 100, false);
    // Positive control: the SAME bytes but owned by the real subledger program.
    let legit = Pubkey::new_unique();
    set_position(&mut svm, &legit, &stub_sub, &genesis_pool, &attacker.pubkey(), 100, 100, false);

    let reg = |linked: &Pubkey| {
        let stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), attacker.pubkey().as_ref()], &rd_id()).0;
        Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false),
            AccountMeta::new_readonly(attacker.pubkey(), true), AccountMeta::new_readonly(attacker.pubkey(), false),
            AccountMeta::new_readonly(*linked, false), AccountMeta::new(stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 1u8] }
    };
    assert!(send(&mut svm, &payer, &[reg(&cosplay)], &[&attacker]).is_err(),
        "an insurance linked NOT owned by the subledger program must reject (type-cosplay, lib.rs:571)");
    // Same bytes, correct owner -> registers. Confirms the owner-type check is the sole difference.
    let r = send(&mut svm, &payer, &[reg(&legit)], &[&attacker]);
    assert!(r.is_ok(), "a subledger-owned position with identical bytes registers normally: {:?}", r);
}

// CROSS-MARKET RESIDUAL FARMING (LOF, finding HI): the residual cohort rewards backing LOSS in the
// genesis market. register_start binds the ledger's authority to `owner` (GY) but, without scoping,
// any backing ledger from ANY OTHER percolator market (authority==attacker) could be registered to
// farm this genesis's residual COIN, diluting legit genesis-market backers. When config.market_group
// is set, register_start must reject a ledger whose market_group != config.market_group.
#[test]
fn register_residual_rejects_a_ledger_from_a_foreign_market() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_market = Pubkey::new_unique();
    let foreign_market = Pubkey::new_unique();

    // rd init: insurance_bps=0; append default subledger_pool then the genesis market_group (HI).
    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    d.extend_from_slice(&[0u8; 32]); // subledger_pool = default (no insurance cohort)
    d.extend_from_slice(genesis_market.as_ref()); // market_group (HI)
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let attacker = Keypair::new();
    let attacker_pk = attacker.pubkey();
    svm.airdrop(&attacker_pk, 1_000_000_000).unwrap();
    let foreign_ledger = Pubkey::new_unique();
    let own_ledger = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    // Backing ledger crafted inline (market_group@16, authority@48 = attacker, percolator-owned).
    let mk = |svm: &mut LiteSVM, key: &Pubkey, market: &Pubkey| {
        let mut data = vec![0u8; 240];
        data[16..48].copy_from_slice(market.as_ref());
        data[48..80].copy_from_slice(attacker_pk.as_ref());
        svm.set_account(*key, Account { lamports: 1_000_000_000, data, owner: stub_perc, executable: false, rent_epoch: 0 }).unwrap();
    };
    mk(&mut svm, &foreign_ledger, &foreign_market);
    mk(&mut svm, &own_ledger, &genesis_market);

    let reg = |linked: &Pubkey| {
        let stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), attacker_pk.as_ref()], &rd_id()).0;
        Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false),
            AccountMeta::new_readonly(attacker_pk, true), AccountMeta::new_readonly(attacker_pk, false),
            AccountMeta::new_readonly(*linked, false), AccountMeta::new(stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 0u8] }
    };
    // Foreign-market ledger must be rejected (scoping).
    let r = send(&mut svm, &payer, &[reg(&foreign_ledger)], &[&attacker]);
    assert!(r.is_err(), "a backing ledger from a foreign market must be rejected (HI scoping)");
    // The genesis-market ledger registers fine (positive control).
    let r = send(&mut svm, &payer, &[reg(&own_ledger)], &[&attacker]);
    assert!(r.is_ok(), "a genesis-market ledger registers normally: {:?}", r);
}

// INIT FRONT-RUN SQUAT (DOS, finding HC): rd_config is a permissionless, canonical-per-coin_mint
// PDA, and init stored `distribution_config` as a FREE param with no validation (genesis-vote binds
// its own, finding R — the residual decider was missing the parity). An attacker front-running rd
// init could wire a FOREIGN distribution_config; since rd_config can't be re-initialized, seal would
// forever target the attacker's config and the canonical COIN-holding distribution could never be
// sealed -> the genesis COIN distribution is bricked (DOS). init must bind distribution_config to the
// canonical PDA(["dist_config", coin_mint, rd_config], distribution_program).
#[test]
fn rd_init_rejects_a_noncanonical_distribution_config() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();

    let foreign_dist = Pubkey::new_unique(); // NOT PDA(["dist_config", coin_mint, rd_config])
    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    let r = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(foreign_dist, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]);
    assert!(r.is_err(), "rd init must reject a non-canonical distribution_config (seal-redirect squat)");
}

// INIT FRONT-RUN SQUAT via FAKE distribution program (DOS, finding HK): HC binds distribution_config
// to the canonical PDA *under the passed distribution_program*. A front-runner could pass a FAKE
// program, derive a canonical-looking config under it, squat rd_config, and brick the real COIN
// distribution at seal (rd would CPI the fake program). init must pin the real distribution program.
#[test]
fn rd_init_rejects_a_fake_distribution_program() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let fake_dist = Pubkey::new_unique(); // attacker-controlled "distribution program"
    // The canonical config UNDER the fake program (what HC would accept if the program weren't pinned).
    let fake_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &fake_dist).0;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();

    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    let r = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(fake_dist, false),
        AccountMeta::new_readonly(fake_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]);
    assert!(r.is_err(), "rd init must reject a fake distribution program (HK pin)");
}

// DUPLICATE-STAKE OVER-ALLOCATION (LOF, finding HA): seal re-derives each proposal entry from a
// passed stake, but it must also ensure each stake is used AT MOST ONCE. distribution::claim is
// per-INDEX (a recipient appearing at two indices claims both). So a hostile cranker could build a
// proposal that DUPLICATES a high-value stake (within the supply headroom left by omitting other
// stakes) and have its recipient claim every copy — stealing the omitted stakes' share of the fixed
// COIN pool. Here two equal backers each deserve 500k of a 1M supply; the cranker omits backer2 and
// lists backer1 TWICE (500k + 500k = 1M, under the cap), passing s1 twice. seal must reject it.
#[test]
fn seal_rejects_a_duplicated_stake_over_allocation() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    svm.add_program_from_file(dist_id(), dist_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let dist_vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &dist_vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(dist_config, false),
        AccountMeta::new_readonly(dist_vault, false), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("dist init");
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let b1 = Keypair::new(); let b2 = Keypair::new();
    let l1 = Pubkey::new_unique(); let l2 = Pubkey::new_unique();
    let s1 = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), b1.pubkey().as_ref()], &rd_id()).0;
    let s2 = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), b2.pubkey().as_ref()], &rd_id()).0;
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &l1, &stub_perc, &b1.pubkey(), 0, 0);
    set_backing_ledger(&mut svm, &l2, &stub_perc, &b2.pubkey(), 0, 0);
    for (owner, linked, stake) in [(&b1,&l1,&s1),(&b2,&l2,&s2)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*linked, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 0u8] }], &[owner]).expect("register");
    }
    // equal residual -> equal points -> each deserves 500k of 1M.
    set_slot(&mut svm, 1_100);
    set_backing_ledger(&mut svm, &l1, &stub_perc, &b1.pubkey(), 1_000, 1_000_000);
    set_backing_ledger(&mut svm, &l2, &stub_perc, &b2.pubkey(), 1_000, 1_000_000);
    for (stake, linked) in [(&s1,&l1),(&s2,&l2)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*linked, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }

    // MALICIOUS proposal: omit b2, list b1 TWICE (500k + 500k = 1M, fits the supply cap).
    set_slot(&mut svm, 2_001);
    let proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &1u64.to_le_bytes()], &dist_id()).0;
    let mut cp = vec![1u8]; cp.extend_from_slice(&1u64.to_le_bytes()); cp.extend_from_slice(&2u32.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: cp }], &[]).expect("create proposal");
    let mut ae = vec![2u8]; ae.extend_from_slice(&2u32.to_le_bytes());
    ae.extend_from_slice(b1.pubkey().as_ref()); ae.extend_from_slice(&500_000u64.to_le_bytes());
    ae.extend_from_slice(b1.pubkey().as_ref()); ae.extend_from_slice(&500_000u64.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false),
    ], data: ae }], &[]).expect("append");

    // seal passing s1 TWICE — must be rejected (a stake may back at most one entry).
    let r = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new(dist_config, false), AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(s1, false), AccountMeta::new_readonly(s1, false),
    ], data: vec![3u8] }], &[]);
    assert!(r.is_err(), "seal must reject a proposal that duplicates a stake (over-allocation theft)");
}

// INCOMPLETE-SEAL GRIEF (governance LOF/DOS, finding HD): seal is permissionless and one-shot. A
// hostile cranker can front-run with a proposal that OMITS some crystallized residual LPs and seal
// it irreversibly — the omitted LPs then get 0 COIN (burned as unclaimed) while the included parties'
// relative governance inflates. The residual cohort has no forfeiture, so seal must require every
// residual stake's points to be represented (sum of sealed residual points == config.total_points).
#[test]
fn seal_rejects_an_incomplete_residual_proposal_that_omits_an_lp() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    svm.add_program_from_file(dist_id(), dist_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let dist_vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &dist_vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(dist_config, false),
        AccountMeta::new_readonly(dist_vault, false), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("dist init");
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let b1 = Keypair::new(); let b2 = Keypair::new();
    let l1 = Pubkey::new_unique(); let l2 = Pubkey::new_unique();
    let s1 = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), b1.pubkey().as_ref()], &rd_id()).0;
    let s2 = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), b2.pubkey().as_ref()], &rd_id()).0;
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &l1, &stub_perc, &b1.pubkey(), 0, 0);
    set_backing_ledger(&mut svm, &l2, &stub_perc, &b2.pubkey(), 0, 0);
    for (owner, linked, stake) in [(&b1,&l1,&s1),(&b2,&l2,&s2)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*linked, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 0u8] }], &[owner]).expect("register");
    }
    set_slot(&mut svm, 1_100);
    set_backing_ledger(&mut svm, &l1, &stub_perc, &b1.pubkey(), 1_000, 1_000_000);
    set_backing_ledger(&mut svm, &l2, &stub_perc, &b2.pubkey(), 1_000, 1_000_000);
    for (stake, linked) in [(&s1,&l1),(&s2,&l2)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*linked, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }

    // INCOMPLETE proposal: omit b2, list only b1 (its honest 500k half). Without the completeness
    // check this seals, burning b2's 500k COIN and inflating b1's relative governance.
    set_slot(&mut svm, 2_001);
    let proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &1u64.to_le_bytes()], &dist_id()).0;
    let mut cp = vec![1u8]; cp.extend_from_slice(&1u64.to_le_bytes()); cp.extend_from_slice(&1u32.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: cp }], &[]).expect("create proposal");
    let mut ae = vec![2u8]; ae.extend_from_slice(&1u32.to_le_bytes());
    ae.extend_from_slice(b1.pubkey().as_ref()); ae.extend_from_slice(&500_000u64.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false),
    ], data: ae }], &[]).expect("append");

    let r = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new(dist_config, false), AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(s1, false),
    ], data: vec![3u8] }], &[]);
    assert!(r.is_err(), "seal must reject an incomplete residual proposal that omits a crystallized LP");
}

// INSURANCE PARTIAL-WITHDRAW OVER-CLAIM (LOF, finding HE): an insurance position only sets
// `withdrawn` on FULL exit; a PARTIAL withdraw reduces principal but keeps withdrawn=false. seal
// used the stale crystallized `stake.points` (only zeroing on the withdrawn flag), so a depositor
// could crystallize at full principal, partially withdraw (recovering capital via the with-surplus
// share exit), and still claim COIN for capital no longer at risk — diluting honest depositors.
// seal must cap each insurance amount by the LIVE position (min of crystallized and live points).
#[test]
fn seal_caps_insurance_amount_by_the_live_position_after_partial_withdraw() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    svm.add_program_from_file(dist_id(), dist_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let dist_vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &dist_vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(dist_config, false),
        AccountMeta::new_readonly(dist_vault, false), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("dist init");
    // insurance_bps = 10000 -> the whole supply is the insurance cohort (isolates this path).
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&10_000u16.to_le_bytes());
    d.extend_from_slice(genesis_pool.as_ref()); // subledger_pool (HG)
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let alice = Keypair::new(); let bob = Keypair::new();
    let a_pos = Pubkey::new_unique(); let bob_pos = Pubkey::new_unique();
    let s_alice = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    let s_bob = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), bob.pubkey().as_ref()], &rd_id()).0;
    set_slot(&mut svm, 100);
    set_position(&mut svm, &a_pos, &stub_sub, &genesis_pool, &alice.pubkey(), 1_000, 100, false);
    set_position(&mut svm, &bob_pos, &stub_sub, &genesis_pool, &bob.pubkey(), 1_000, 100, false);
    for (owner, linked, stake) in [(&alice,&a_pos,&s_alice),(&bob,&bob_pos,&s_bob)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*linked, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 1u8] }], &[owner]).expect("register insurance");
    }
    // Both crystallize at slot 1100 with full principal -> equal stale points (1000*log2(1000)=9000).
    set_slot(&mut svm, 1_100);
    for (stake, linked) in [(&s_alice,&a_pos),(&s_bob,&bob_pos)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*linked, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }
    // BOB PARTIALLY withdraws: principal 1000 -> 1, but withdrawn stays false (not a full exit). He
    // does NOT re-crystallize, so s_bob.points is stale-high (9000).
    set_position(&mut svm, &bob_pos, &stub_sub, &genesis_pool, &bob.pubkey(), 1, 100, false);

    set_slot(&mut svm, 2_001);
    let proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &1u64.to_le_bytes()], &dist_id()).0;
    let mut cp = vec![1u8]; cp.extend_from_slice(&1u64.to_le_bytes()); cp.extend_from_slice(&2u32.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: cp }], &[]).expect("create proposal");
    // STALE proposal: bob claims his stale half (500k) as if still fully deposited.
    let mut ae = vec![2u8]; ae.extend_from_slice(&2u32.to_le_bytes());
    ae.extend_from_slice(alice.pubkey().as_ref()); ae.extend_from_slice(&500_000u64.to_le_bytes());
    ae.extend_from_slice(bob.pubkey().as_ref()); ae.extend_from_slice(&500_000u64.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false),
    ], data: ae }], &[]).expect("append");

    let r = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new(dist_config, false), AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(s_alice, false), AccountMeta::new_readonly(a_pos, false),
        AccountMeta::new_readonly(s_bob, false), AccountMeta::new_readonly(bob_pos, false),
    ], data: vec![3u8] }], &[]);
    assert!(r.is_err(), "seal must reject bob's stale-points over-claim after a partial withdraw (cap by live position)");
}

// SOFT-VETO ENFORCEMENT (forfeiture cannot be overridden by the cranker, finding HQ): an insurance
// depositor who FULLY exits (subledger sets withdrawn=true) forfeits its COIN. insurance_cohort_split
// shows the HONEST cranker omits the exited depositor; this is the ADVERSARIAL dual — a malicious
// cranker that INCLUDES the exited depositor with a non-zero amount must be REJECTED (seal forces the
// live-withdrawn entry to amount 0, so any non-zero entry mismatches). Exercises the `if withdrawn { 0 }`
// branch that the omit-path test never reaches.
#[test]
fn seal_rejects_a_proposal_that_pays_a_forfeited_insurance_depositor() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    svm.add_program_from_file(dist_id(), dist_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let dist_vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &dist_vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(dist_config, false),
        AccountMeta::new_readonly(dist_vault, false), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("dist init");
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&10_000u16.to_le_bytes());
    d.extend_from_slice(genesis_pool.as_ref());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let alice = Keypair::new(); let bob = Keypair::new();
    let a_pos = Pubkey::new_unique(); let bob_pos = Pubkey::new_unique();
    let s_alice = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    let s_bob = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), bob.pubkey().as_ref()], &rd_id()).0;
    set_slot(&mut svm, 100);
    set_position(&mut svm, &a_pos, &stub_sub, &genesis_pool, &alice.pubkey(), 1_000, 100, false);
    set_position(&mut svm, &bob_pos, &stub_sub, &genesis_pool, &bob.pubkey(), 1_000, 100, false);
    for (owner, linked, stake) in [(&alice,&a_pos,&s_alice),(&bob,&bob_pos,&s_bob)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*linked, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 1u8] }], &[owner]).expect("register insurance");
    }
    set_slot(&mut svm, 1_100);
    for (stake, linked) in [(&s_alice,&a_pos),(&s_bob,&bob_pos)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*linked, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }
    // BOB FULLY EXITS: subledger zeroes principal and sets withdrawn -> his COIN is forfeited.
    set_position(&mut svm, &bob_pos, &stub_sub, &genesis_pool, &bob.pubkey(), 0, 100, true);

    set_slot(&mut svm, 2_001);
    let proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &1u64.to_le_bytes()], &dist_id()).0;
    let mut cp = vec![1u8]; cp.extend_from_slice(&1u64.to_le_bytes()); cp.extend_from_slice(&2u32.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: cp }], &[]).expect("create proposal");
    // MALICIOUS proposal: pay the EXITED bob his stale half anyway.
    let mut ae = vec![2u8]; ae.extend_from_slice(&2u32.to_le_bytes());
    ae.extend_from_slice(alice.pubkey().as_ref()); ae.extend_from_slice(&500_000u64.to_le_bytes());
    ae.extend_from_slice(bob.pubkey().as_ref()); ae.extend_from_slice(&500_000u64.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false),
    ], data: ae }], &[]).expect("append");

    let r = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new(dist_config, false), AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(s_alice, false), AccountMeta::new_readonly(a_pos, false),
        AccountMeta::new_readonly(s_bob, false), AccountMeta::new_readonly(bob_pos, false),
    ], data: vec![3u8] }], &[]);
    assert!(r.is_err(), "seal must reject a proposal that pays a fully-exited (forfeited) insurance depositor");
}

// FORFEITED-SHARE REDISTRIBUTION (cross-cohort value shift, finding HR): a forfeited insurance
// depositor's share must be BURNED (stays in insurance_total_points -> unclaimed -> burn), not
// REDISTRIBUTED to the surviving insurance depositors. crystallize re-derived a position's points via
// subtract-old/add-new, so RE-crystallizing a withdrawn co-depositor's stake removed its points from
// the denominator, inflating every survivor's share — a permissionless cranker could thus shift the
// forfeited share from the burn (deflation to ALL COIN holders) to the insurance cohort. Fix:
// crystallize is a NO-OP on a withdrawn position (its points stay in the denominator -> burned).
#[test]
fn recrystallizing_a_forfeited_stake_cannot_redistribute_its_share() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    svm.add_program_from_file(dist_id(), dist_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let dist_vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &dist_vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(dist_config, false),
        AccountMeta::new_readonly(dist_vault, false), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("dist init");
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&10_000u16.to_le_bytes());
    d.extend_from_slice(genesis_pool.as_ref());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let alice = Keypair::new(); let bob = Keypair::new();
    let a_pos = Pubkey::new_unique(); let bob_pos = Pubkey::new_unique();
    let s_alice = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    let s_bob = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), bob.pubkey().as_ref()], &rd_id()).0;
    set_slot(&mut svm, 100);
    set_position(&mut svm, &a_pos, &stub_sub, &genesis_pool, &alice.pubkey(), 1_000, 100, false);
    set_position(&mut svm, &bob_pos, &stub_sub, &genesis_pool, &bob.pubkey(), 1_000, 100, false);
    for (owner, linked, stake) in [(&alice,&a_pos,&s_alice),(&bob,&bob_pos,&s_bob)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*linked, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 1u8] }], &[owner]).expect("register insurance");
    }
    set_slot(&mut svm, 1_100);
    for (stake, linked) in [(&s_alice,&a_pos),(&s_bob,&bob_pos)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*linked, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }
    // bob FULLY exits (forfeits). A hostile cranker then RE-crystallizes bob's withdrawn stake to try
    // to drop bob from insurance_total_points and redistribute his half to alice.
    set_position(&mut svm, &bob_pos, &stub_sub, &genesis_pool, &bob.pubkey(), 0, 100, true);
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(s_bob, false), AccountMeta::new_readonly(bob_pos, false),
    ], data: vec![2u8] }], &[]).expect("re-crystallize the withdrawn bob (no-op after fix)");

    set_slot(&mut svm, 2_001);
    let proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &1u64.to_le_bytes()], &dist_id()).0;
    let mut cp = vec![1u8]; cp.extend_from_slice(&1u64.to_le_bytes()); cp.extend_from_slice(&1u32.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: cp }], &[]).expect("create proposal");
    // Proposal pays alice the FULL supply (only possible if bob was dropped from the denominator).
    let mut ae = vec![2u8]; ae.extend_from_slice(&1u32.to_le_bytes());
    ae.extend_from_slice(alice.pubkey().as_ref()); ae.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false),
    ], data: ae }], &[]).expect("append");

    let r = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new(dist_config, false), AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(s_alice, false), AccountMeta::new_readonly(a_pos, false),
    ], data: vec![3u8] }], &[]);
    assert!(r.is_err(), "re-crystallizing a forfeited stake must NOT let alice claim bob's burned half (alice fair share is supply/2)");
}

// INSURANCE INCOMPLETE-SEAL GRIEF (governance LOF, finding HX; the insurance dual of HD): seal is
// PERMISSIONLESS and one-shot. The residual cohort is protected by HD (sealed_residual_points must
// equal config.total_points), but the insurance cohort had NO symmetric completeness check — a
// documented asymmetry. That let a malicious cranker FRONT-RUN the honest seal with a proposal that
// silently OMITS a non-forfeited (still-deposited, crystallized) insurance depositor: the proposal
// pays every OTHER party their exact deterministic amount (so each per-entry check passes) but leaves
// the victim out entirely. Once sealed it is irreversible, so the omitted depositor gets 0 COIN
// forever (their crystallized share burned) with no recourse. Insurance points materialize only at
// crystallize (register sets points=0), so the fix protects exactly the depositors who crystallized
// before the seal — symmetric with HD. Forfeited (withdrawn) depositors legitimately get amount 0 and
// distribution::append rejects 0-amount entries, so they cannot be proposal entries; the cranker must
// instead pass them as trailing (stake, position) extras whose points still count toward the
// insurance denominator (HR keeps them -> burned, not paid). The completeness check then forces EVERY
// crystallized insurance stake to be represented (entry or forfeited-extra).
#[test]
fn seal_rejects_an_incomplete_insurance_proposal_that_omits_a_depositor() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    svm.add_program_from_file(dist_id(), dist_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let dist_vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &dist_vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(dist_config, false),
        AccountMeta::new_readonly(dist_vault, false), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("dist init");
    // insurance_bps = 10000 -> the whole supply is the insurance cohort (isolates this path).
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&10_000u16.to_le_bytes());
    d.extend_from_slice(genesis_pool.as_ref());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let alice = Keypair::new(); let bob = Keypair::new();
    let a_pos = Pubkey::new_unique(); let bob_pos = Pubkey::new_unique();
    let s_alice = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    let s_bob = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), bob.pubkey().as_ref()], &rd_id()).0;
    set_slot(&mut svm, 100);
    // BOTH depositors are active and identical -> each is owed half the cohort.
    set_position(&mut svm, &a_pos, &stub_sub, &genesis_pool, &alice.pubkey(), 1_000, 100, false);
    set_position(&mut svm, &bob_pos, &stub_sub, &genesis_pool, &bob.pubkey(), 1_000, 100, false);
    for (owner, linked, stake) in [(&alice,&a_pos,&s_alice),(&bob,&bob_pos,&s_bob)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*linked, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 1u8] }], &[owner]).expect("register insurance");
    }
    // Both crystallize (hold=1000 -> 1000*log2(1000)=9000 pts each -> insurance_total = 18000).
    set_slot(&mut svm, 1_100);
    for (stake, linked) in [(&s_alice,&a_pos),(&s_bob,&bob_pos)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*linked, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }

    set_slot(&mut svm, 2_001);
    let proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &1u64.to_le_bytes()], &dist_id()).0;
    let mut cp = vec![1u8]; cp.extend_from_slice(&1u64.to_le_bytes()); cp.extend_from_slice(&1u32.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: cp }], &[]).expect("create proposal");
    // GRIEF: the cranker pays alice her EXACT fair half (500k -> the per-entry check passes) but
    // OMITS the equally-entitled, still-deposited bob entirely. Without insurance completeness this
    // seals irreversibly and bob -- who never withdrew -- gets 0 COIN forever.
    let mut ae = vec![2u8]; ae.extend_from_slice(&1u32.to_le_bytes());
    ae.extend_from_slice(alice.pubkey().as_ref()); ae.extend_from_slice(&500_000u64.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false),
    ], data: ae }], &[]).expect("append");

    // seal with ONLY alice's (stake, position) -- bob is omitted everywhere. sealed_insurance_points
    // (alice 9000) != insurance_total_points (18000) -> must reject.
    let r = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new(dist_config, false), AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(s_alice, false), AccountMeta::new_readonly(a_pos, false),
    ], data: vec![3u8] }], &[]);
    assert!(r.is_err(), "seal must reject an insurance proposal that omits a non-forfeited (crystallized) depositor");

    // SANITY (non-tautology): the COMPLETE proposal that pays both halves DOES seal -- proving the
    // rejection above is the omission, not some unrelated failure. (Both seal accounts are entries.)
    let proposal2 = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &2u64.to_le_bytes()], &dist_id()).0;
    let mut cp = vec![1u8]; cp.extend_from_slice(&2u64.to_le_bytes()); cp.extend_from_slice(&2u32.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal2, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: cp }], &[]).expect("create proposal2");
    let mut ae = vec![2u8]; ae.extend_from_slice(&2u32.to_le_bytes());
    ae.extend_from_slice(alice.pubkey().as_ref()); ae.extend_from_slice(&500_000u64.to_le_bytes());
    ae.extend_from_slice(bob.pubkey().as_ref()); ae.extend_from_slice(&500_000u64.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal2, false),
    ], data: ae }], &[]).expect("append2");
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new(dist_config, false), AccountMeta::new(proposal2, false),
        AccountMeta::new_readonly(s_alice, false), AccountMeta::new_readonly(a_pos, false),
        AccountMeta::new_readonly(s_bob, false), AccountMeta::new_readonly(bob_pos, false),
    ], data: vec![3u8] }], &[]).expect("the complete insurance proposal seals");
}

// DEFAULT-RECIPIENT SEAL DOS (finding IK; a single-stake sharpening of IG): register_start stored the
// stake's `recipient` UNVALIDATED. A stake registered with recipient = Pubkey::default(), once
// crystallized, has its points in config.total_points (residual) / insurance_total_points, so HD/HX
// completeness REQUIRES it represented in the seal. But its only possible entry is (default, amount),
// which distribution::append REJECTS (default-pubkey guard); and (being active, not withdrawn) it
// cannot be a forfeited extra either. So a SINGLE dust stake with a default recipient makes
// completeness permanently unsatisfiable -> seal DOS, on ANY genesis size (no flood needed). A default
// recipient is never legitimate (no one sends COIN to the null address), so register_start now rejects
// it -> every stake can always form a valid-recipient entry.
#[test]
fn register_rejects_a_default_recipient() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&5_000u64.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let owner = Keypair::new();
    svm.airdrop(&owner.pubkey(), 1_000_000_000).unwrap();
    let ledger = Pubkey::new_unique();
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &ledger, &stub_perc, &owner.pubkey(), 0, 0);
    let stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), owner.pubkey().as_ref()], &rd_id()).0;

    // register a residual stake naming the NULL address as the COIN recipient (owner signs).
    let r = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
        AccountMeta::new_readonly(Pubkey::default(), false), AccountMeta::new_readonly(ledger, false), AccountMeta::new(stake, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: vec![1u8, 0u8] }], &[&owner]);
    assert!(r.is_err(), "register must reject a default-pubkey recipient (would make seal completeness unsatisfiable)");

    // Boundary: a real recipient is accepted (the reject is the default-guard, not an unrelated failure).
    let r = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
        AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(ledger, false), AccountMeta::new(stake, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: vec![1u8, 0u8] }], &[&owner]);
    assert!(r.is_ok(), "a real recipient is accepted: {:?}", r);
}

// AMOUNT-ROUNDS-TO-0 SEAL LIVENESS (finding IL fix): a crystallized stake whose deterministic amount
// rounds to 0 (when total_points > supply for its points) cannot be a distribution entry (append
// rejects amount==0) and, if active, was rejected as a forfeited extra (!withdrawn) -> HD/HX
// completeness was UNSATISFIABLE, so even an HONEST genesis with a "minnow" alongside a "whale" could
// not seal (no attacker needed). FIX: the completeness extras now accept ANY zero-pay stake (forfeited
// OR dust) in EITHER cohort, counted toward completeness, paid nothing; the amount==0 check is the
// safety (a stake owed a nonzero amount is rejected as an extra -> must be a paid entry).
#[test]
fn seal_accepts_a_dust_stake_that_rounds_to_zero_as_a_completeness_extra() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    svm.add_program_from_file(dist_id(), dist_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    // SMALL supply so the minnow's points round to a 0 amount against the whale-dominated total.
    let supply = 1_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let dist_vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &dist_vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(dist_config, false),
        AccountMeta::new_readonly(dist_vault, false), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("dist init");
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&10_000u16.to_le_bytes()); d.extend_from_slice(genesis_pool.as_ref());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let whale = Keypair::new(); let minnow = Keypair::new();
    let w_pos = Pubkey::new_unique(); let m_pos = Pubkey::new_unique();
    let s_whale = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), whale.pubkey().as_ref()], &rd_id()).0;
    let s_minnow = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), minnow.pubkey().as_ref()], &rd_id()).0;
    set_slot(&mut svm, 100);
    set_position(&mut svm, &w_pos, &stub_sub, &genesis_pool, &whale.pubkey(), 1_000_000, 100, false);
    set_position(&mut svm, &m_pos, &stub_sub, &genesis_pool, &minnow.pubkey(), 1, 100, false);
    for (owner, linked, stake) in [(&whale,&w_pos,&s_whale),(&minnow,&m_pos,&s_minnow)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*linked, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 1u8] }], &[owner]).expect("register");
    }
    // hold=1000 -> floor_log2=9. whale 1e6*9=9_000_000 pts; minnow 1*9=9 pts; total 9_000_009.
    // whale amount = 1000*9_000_000/9_000_009 = 999; minnow = 1000*9/9_000_009 = 0 (rounds to 0).
    set_slot(&mut svm, 1_100);
    for (stake, linked) in [(&s_whale,&w_pos),(&s_minnow,&m_pos)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*linked, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }

    set_slot(&mut svm, 2_001);
    let proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &1u64.to_le_bytes()], &dist_id()).0;
    let mut cp = vec![1u8]; cp.extend_from_slice(&1u64.to_le_bytes()); cp.extend_from_slice(&1u32.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: cp }], &[]).expect("create proposal");
    // Only the whale is a paid entry (999); the minnow rounds to 0 and CANNOT be an entry.
    let mut ap = vec![2u8]; ap.extend_from_slice(&1u32.to_le_bytes());
    ap.extend_from_slice(whale.pubkey().as_ref()); ap.extend_from_slice(&999u64.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false),
    ], data: ap }], &[]).expect("append");
    // seal: whale entry + the minnow as a zero-pay (dust) completeness EXTRA. Without the IL fix the
    // extras loop rejects the active minnow (!withdrawn) -> completeness 9_000_000 != 9_000_009 -> DOS.
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new(dist_config, false), AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(s_whale, false), AccountMeta::new_readonly(w_pos, false),
        AccountMeta::new_readonly(s_minnow, false), AccountMeta::new_readonly(m_pos, false),
    ], data: vec![3u8] }], &[]).expect("seal completes with the dust minnow as a zero-pay extra");
}

// IM SAFETY DUAL (a cranker must not zero an owed depositor via the new zero-pay extras): finding IM
// lets a stake whose amount is 0 (forfeiture/dust) be a completeness EXTRA paid nothing. The safety
// is the `amount != 0 -> reject` check: a depositor OWED a nonzero amount cannot be slipped in as a
// free extra (which would satisfy completeness while paying them 0 = a grief). This pins that: two
// equal active insurance depositors; a proposal pays alice but passes the owed bob as a zero-pay extra
// -> the seal must REJECT (bob's amount is nonzero, so he must be a paid entry).
#[test]
fn seal_rejects_zeroing_an_owed_depositor_via_a_free_extra() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    svm.add_program_from_file(dist_id(), dist_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let dist_vault = create_token_account(&mut svm, &payer, &coin_mint, &dist_config);
    mint_to(&mut svm, &payer, &coin_mint, &mint_auth, &dist_vault, supply);
    revoke_mint(&mut svm, &payer, &coin_mint, &mint_auth);

    let mut d = vec![0u8]; d.extend_from_slice(&1_000_000u64.to_le_bytes()); d.extend_from_slice(&supply.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(dist_config, false),
        AccountMeta::new_readonly(dist_vault, false), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("dist init");
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&10_000u16.to_le_bytes()); d.extend_from_slice(genesis_pool.as_ref());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let alice = Keypair::new(); let bob = Keypair::new();
    let a_pos = Pubkey::new_unique(); let b_pos = Pubkey::new_unique();
    let s_alice = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), alice.pubkey().as_ref()], &rd_id()).0;
    let s_bob = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), bob.pubkey().as_ref()], &rd_id()).0;
    set_slot(&mut svm, 100);
    set_position(&mut svm, &a_pos, &stub_sub, &genesis_pool, &alice.pubkey(), 1_000, 100, false);
    set_position(&mut svm, &b_pos, &stub_sub, &genesis_pool, &bob.pubkey(), 1_000, 100, false);
    for (owner, linked, stake) in [(&alice,&a_pos,&s_alice),(&bob,&b_pos,&s_bob)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(owner.pubkey(), false), AccountMeta::new_readonly(*linked, false), AccountMeta::new(*stake, false),
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: vec![1u8, 1u8] }], &[owner]).expect("register");
    }
    set_slot(&mut svm, 1_100);
    for (stake, linked) in [(&s_alice,&a_pos),(&s_bob,&b_pos)] {
        send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(*stake, false), AccountMeta::new_readonly(*linked, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }
    // Both are owed 500k (9000/18000 of the 1M insurance supply). The cranker pays alice but tries to
    // zero bob by slipping him in as a free completeness extra.
    set_slot(&mut svm, 2_001);
    let proposal = Pubkey::find_program_address(&[b"dist_proposal", dist_config.as_ref(), &1u64.to_le_bytes()], &dist_id()).0;
    let mut cp = vec![1u8]; cp.extend_from_slice(&1u64.to_le_bytes()); cp.extend_from_slice(&1u32.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: cp }], &[]).expect("create proposal");
    let mut ap = vec![2u8]; ap.extend_from_slice(&1u32.to_le_bytes());
    ap.extend_from_slice(alice.pubkey().as_ref()); ap.extend_from_slice(&500_000u64.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: dist_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(dist_config, false), AccountMeta::new(proposal, false),
    ], data: ap }], &[]).expect("append");
    let r = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new(dist_config, false), AccountMeta::new(proposal, false),
        AccountMeta::new_readonly(s_alice, false), AccountMeta::new_readonly(a_pos, false),
        AccountMeta::new_readonly(s_bob, false), AccountMeta::new_readonly(b_pos, false),
    ], data: vec![3u8] }], &[]);
    assert!(r.is_err(), "seal must reject zeroing an owed (nonzero-amount) depositor via a free completeness extra");
}

// SUBSTITUTED-LEDGER CRYSTALLIZE THEFT (finding IP): crystallize reads the residual counters
// (cumulative_loss, total_earnings) from a PASSED backing_ledger account. It binds that account to the
// stake's registered ledger (`stake.backing_ledger != backing_ledger.key -> reject`). Without that
// bind, an attacker could crystallize their OWN (registered against a clean ledger) stake against a
// DIFFERENT, high-loss ledger -> total_res = foreign_loss - own_snap inflates -> points balloon ->
// over-allocation of the fixed COIN supply (theft / dilution of honest backers). This pins the bind.
#[test]
fn crystallize_rejects_a_substituted_backing_ledger() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let supply = 1_000_000u64;
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&80u16.to_le_bytes());
    d.extend_from_slice(&5_000u64.to_le_bytes()); d.extend_from_slice(&0u16.to_le_bytes());
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");

    let attacker = Keypair::new();
    svm.airdrop(&attacker.pubkey(), 1_000_000_000).unwrap();
    let own_ledger = Pubkey::new_unique();    // the attacker's OWN ledger (registered), clean.
    let foreign_ledger = Pubkey::new_unique(); // a DIFFERENT, high-loss ledger.
    let stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), attacker.pubkey().as_ref()], &rd_id()).0;
    set_slot(&mut svm, 100);
    set_backing_ledger(&mut svm, &own_ledger, &stub_perc, &attacker.pubkey(), 0, 0);
    // The foreign ledger has a big loss + fees (would mint ~9000 points if read). Its authority is
    // irrelevant here -- crystallize never checks the ledger's authority, only the key-bind.
    set_backing_ledger(&mut svm, &foreign_ledger, &stub_perc, &attacker.pubkey(), 1_000, 1_000_000);
    // register against the OWN (clean) ledger.
    send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(attacker.pubkey(), true),
        AccountMeta::new_readonly(attacker.pubkey(), false), AccountMeta::new_readonly(own_ledger, false), AccountMeta::new(stake, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: vec![1u8, 0u8] }], &[&attacker]).expect("register against own clean ledger");

    set_slot(&mut svm, 1_100);
    // ATTACK: crystallize the stake against the FOREIGN high-loss ledger to inflate points.
    let r = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(stake, false), AccountMeta::new_readonly(foreign_ledger, false),
    ], data: vec![2u8] }], &[]);
    assert!(r.is_err(), "crystallize must reject a backing_ledger != the stake's registered one (substitution theft)");
    // The stake's points are untouched (still 0 -- never crystallized against the foreign loss).
    let pts = u128::from_le_bytes(svm.get_account(&stake).unwrap().data[176..192].try_into().unwrap());
    assert_eq!(pts, 0, "no points minted from the foreign ledger");

    // Boundary: crystallizing against the OWN (registered, clean) ledger is accepted (0 loss -> 0 pts).
    let r = send(&mut svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(stake, false), AccountMeta::new_readonly(own_ledger, false),
    ], data: vec![2u8] }], &[]);
    assert!(r.is_ok(), "crystallize against the registered ledger is accepted: {:?}", r);
}

// RD INIT PARAM-RANGE VALIDATION (finding IQ): the seal computes
// `residual_supply = total_supply - total_supply*insurance_bps/10000`. If insurance_bps > 10000 this
// UNDERFLOWS (insurance_supply > total_supply) -> residual_supply wraps to a huge u64 -> garbage
// residual amounts (cohort point miscalc / over-allocation / seal DOS). rd_init guards it up front
// (insurance_bps > BPS_DENOMINATOR -> reject), alongside total_supply != 0 and fee_support_bps != 0
// (a 0 fee cap would zero every residual point). This pins those guards.
#[test]
fn rd_init_rejects_out_of_range_params() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let stub_perc = Pubkey::new_unique();
    let stub_sub = Pubkey::new_unique();
    let genesis_pool = Pubkey::new_unique();
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(&mut svm, &payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;

    // (total_supply, fee_support_bps, insurance_bps, with_pool) -> build the rd_init data.
    let init = |svm: &mut LiteSVM, supply: u64, fee_bps: u16, ins_bps: u16| -> Result<(), String> {
        let mut d = vec![0u8]; d.extend_from_slice(&supply.to_le_bytes()); d.extend_from_slice(&fee_bps.to_le_bytes());
        d.extend_from_slice(&2_000u64.to_le_bytes()); d.extend_from_slice(&ins_bps.to_le_bytes());
        if ins_bps > 0 { d.extend_from_slice(genesis_pool.as_ref()); }
        send(svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
            AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
            AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
        ], data: d }], &[])
    };

    // insurance_bps > 10000 -> residual_supply would underflow at seal. Rejected.
    assert!(init(&mut svm, 1_000_000, 80, 10_001).is_err(), "insurance_bps > 10000 must be rejected (residual_supply underflow)");
    // total_supply == 0 -> nothing to distribute. Rejected.
    assert!(init(&mut svm, 0, 80, 2_000).is_err(), "total_supply == 0 must be rejected");
    // fee_support_bps == 0 -> the fee cap zeros every residual point. Rejected.
    assert!(init(&mut svm, 1_000_000, 0, 2_000).is_err(), "fee_support_bps == 0 must be rejected");
    // Boundary: insurance_bps == 10000 (all-insurance) is valid and accepted.
    assert!(init(&mut svm, 1_000_000, 80, 10_000).is_ok(), "insurance_bps == 10000 is the valid maximum");
}
