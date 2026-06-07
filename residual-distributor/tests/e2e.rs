//! [branch-only, DO NOT PUSH] e2e: residual-distributor 4-cohort deterministic distribution
//! (10/10/40/40). Insurance + backing reward SUBLEDGER SHARE VALUE (Position.shares — pro-rata with
//! fees, soft-veto on exit); LP + trader reward the percolator PortfolioAccountV16 residual counters
//! (received / crystallized_loss — monotonic, real-loss-backed, un-gameable). Self-service flow:
//! register -> crystallize -> freeze -> claim, against mock dependency accounts at the offset-pinned
//! layouts (tests/offsets.rs pins every offset vs the real percolator/subledger structs).

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
    Pubkey::try_from("D1str1but1on11111111111111111111111111111111").unwrap()
}
fn rd_so() -> String {
    format!("{}/../target/deploy/residual_distributor.so", env!("CARGO_MANIFEST_DIR"))
}

const COHORT_INSURANCE: u8 = 0;
const COHORT_BACKING: u8 = 1;
const COHORT_LP: u8 = 2;
const COHORT_TRADER: u8 = 3;

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

// Mock subledger Position at the pinned offsets: pool@8, owner@40, withdrawn@88, shares@104.
fn set_position(svm: &mut LiteSVM, key: &Pubkey, sub: &Pubkey, pool: &Pubkey, owner: &Pubkey, shares: u128, withdrawn: bool) {
    let mut data = vec![0u8; 160];
    data[8..40].copy_from_slice(pool.as_ref());
    data[40..72].copy_from_slice(owner.as_ref());
    data[88] = withdrawn as u8;
    data[104..120].copy_from_slice(&shares.to_le_bytes());
    svm.set_account(*key, Account { lamports: 1_000_000_000, data, owner: *sub, executable: false, rent_epoch: 0 }).unwrap();
}
// Mock percolator PortfolioAccount at the pinned offsets: market_group@16, owner@116, crystallized@196,
// received@228. market_group is the trusted-Pyth scope the LP/trader cohorts enforce (finding IL).
fn set_portfolio(svm: &mut LiteSVM, key: &Pubkey, perc: &Pubkey, market: &Pubkey, owner: &Pubkey, received: u128, crystallized: u128) {
    let mut data = vec![0u8; 512];
    data[16..48].copy_from_slice(market.as_ref());
    data[116..148].copy_from_slice(owner.as_ref());
    data[196..212].copy_from_slice(&crystallized.to_le_bytes());
    data[228..244].copy_from_slice(&received.to_le_bytes());
    svm.set_account(*key, Account { lamports: 1_000_000_000, data, owner: *perc, executable: false, rent_epoch: 0 }).unwrap();
}

struct Env {
    rd_config: Pubkey,
    coin_mint: Pubkey,
    vault: Pubkey,
    mint_auth: Keypair,
    stub_sub: Pubkey,
    stub_perc: Pubkey,
    ins_pool: Pubkey,
    back_pool: Pubkey,
    market: Pubkey,
    supply: u64,
    emission_end: u64,
    finalize_window: u64,
}

// Init an rd_config (10/10/40/40) with a fully-funded rd-owned COIN vault (the self-service claim vault).
fn setup(svm: &mut LiteSVM, payer: &Keypair, supply: u64) -> Env {
    let emission_end = 2_000u64;
    let finalize_window = 500u64;
    let mint_auth = Keypair::new();
    let coin_mint = create_mint(svm, payer, &mint_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let vault = create_token_account(svm, payer, &coin_mint, &rd_config); // rd-owned claim vault
    mint_to(svm, payer, &coin_mint, &mint_auth, &vault, supply);
    revoke_mint(svm, payer, &coin_mint, &mint_auth);

    let stub_sub = Pubkey::new_unique();
    let stub_perc = Pubkey::new_unique();
    let ins_pool = Pubkey::new_unique();
    let back_pool = Pubkey::new_unique();
    let market = Pubkey::new_unique();
    // wire: supply, emission_end, insurance_bps, backing_bps, lp_bps, finalize_window, ins_pool, back_pool, market
    let mut d = vec![0u8];
    d.extend_from_slice(&supply.to_le_bytes());
    d.extend_from_slice(&emission_end.to_le_bytes());
    d.extend_from_slice(&1_000u16.to_le_bytes()); // insurance 10%
    d.extend_from_slice(&1_000u16.to_le_bytes()); // backing 10%
    d.extend_from_slice(&4_000u16.to_le_bytes()); // lp 40% (trader = remainder 40%)
    d.extend_from_slice(&finalize_window.to_le_bytes());
    d.extend_from_slice(ins_pool.as_ref());
    d.extend_from_slice(back_pool.as_ref());
    d.extend_from_slice(market.as_ref());
    send(svm, payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false),
        AccountMeta::new_readonly(dist_id(), false), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new_readonly(stub_perc, false), AccountMeta::new_readonly(stub_sub, false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");
    Env { rd_config, coin_mint, vault, mint_auth, stub_sub, stub_perc, ins_pool, back_pool, market, supply, emission_end, finalize_window }
}

fn stake_pda(env: &Env, owner: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"rd_stake", env.rd_config.as_ref(), owner.as_ref()], &rd_id()).0
}
fn register(svm: &mut LiteSVM, payer: &Keypair, env: &Env, owner: &Keypair, recipient: &Pubkey, linked: &Pubkey, cohort: u8) -> Result<(), String> {
    let stake = stake_pda(env, &owner.pubkey());
    send(svm, payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(env.rd_config, false),
        AccountMeta::new_readonly(owner.pubkey(), true), AccountMeta::new_readonly(*recipient, false),
        AccountMeta::new_readonly(*linked, false), AccountMeta::new(stake, false),
        AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: vec![1u8, cohort] }], &[owner])
}
// `cranker` triggers crystallize (first account, must sign). Share-value cohorts (insurance/backing)
// require it to be the stake owner (finding KO); LP/trader accept any cranker.
fn crystallize_as(svm: &mut LiteSVM, payer: &Keypair, env: &Env, cranker: &Keypair, owner: &Pubkey, linked: &Pubkey) -> Result<(), String> {
    let stake = stake_pda(env, owner);
    send(svm, payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(cranker.pubkey(), true), AccountMeta::new(env.rd_config, false),
        AccountMeta::new(stake, false), AccountMeta::new_readonly(*linked, false),
    ], data: vec![2u8] }], &[cranker])
}
// Default crystallize: the owner authorizes their own (valid for every cohort).
fn crystallize(svm: &mut LiteSVM, payer: &Keypair, env: &Env, owner: &Keypair, linked: &Pubkey) -> Result<(), String> {
    crystallize_as(svm, payer, env, owner, &owner.pubkey(), linked)
}
fn freeze(svm: &mut LiteSVM, payer: &Keypair, env: &Env) -> Result<(), String> {
    send(svm, payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(env.rd_config, false),
        AccountMeta::new_readonly(env.coin_mint, false), AccountMeta::new(env.vault, false),
    ], data: vec![4u8] }], &[])
}
// claim: insurance/backing append the live subledger position (for the live-share cap); LP/trader don't.
// `cranker` is the claim trigger (first account, must sign). Share-value cohorts (insurance/backing)
// require it to be the stake owner (finding KM); LP/trader accept any cranker. The helper takes the
// cranker keypair explicitly so tests can model both the owner's own claim and a foreign forced claim.
fn claim_as(svm: &mut LiteSVM, payer: &Keypair, env: &Env, cranker: &Keypair, owner: &Pubkey, recipient_ata: &Pubkey, position: Option<&Pubkey>) -> Result<(), String> {
    let stake = stake_pda(env, owner);
    let mut accounts = vec![
        AccountMeta::new(cranker.pubkey(), true), AccountMeta::new_readonly(env.rd_config, false),
        AccountMeta::new(stake, false), AccountMeta::new(env.vault, false),
        AccountMeta::new(*recipient_ata, false), AccountMeta::new_readonly(spl_token::ID, false),
    ];
    if let Some(p) = position { accounts.push(AccountMeta::new_readonly(*p, false)); }
    send(svm, payer, &[Instruction { program_id: rd_id(), accounts, data: vec![5u8] }], &[cranker])
}
// Default claim: the owner authorizes their own claim (valid for every cohort).
fn claim(svm: &mut LiteSVM, payer: &Keypair, env: &Env, owner: &Keypair, recipient_ata: &Pubkey, position: Option<&Pubkey>) -> Result<(), String> {
    claim_as(svm, payer, env, owner, &owner.pubkey(), recipient_ata, position)
}

// HEADLINE: all four cohorts in one genesis, one staker each -> each claims its full cohort_supply
// (10/10/40/40). Insurance/backing from subledger share value; LP/trader from portfolio residual counters.
#[test]
fn full_four_way_split_pays_each_cohort_its_share() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let supply = 1_000_000u64;
    let env = setup(&mut svm, &payer, supply);
    set_slot(&mut svm, 100);

    let (ins, back, lp, trd) = (Keypair::new(), Keypair::new(), Keypair::new(), Keypair::new());
    let ins_pos = Pubkey::new_unique();
    let back_pos = Pubkey::new_unique();
    let lp_pf = Pubkey::new_unique();
    let trd_pf = Pubkey::new_unique();
    // Insurance + backing positions (share value); LP/trader portfolios (residual counters, start 0).
    set_position(&mut svm, &ins_pos, &env.stub_sub, &env.ins_pool, &ins.pubkey(), 500, false);
    set_position(&mut svm, &back_pos, &env.stub_sub, &env.back_pool, &back.pubkey(), 700, false);
    set_portfolio(&mut svm, &lp_pf, &env.stub_perc, &env.market, &lp.pubkey(), 0, 0);
    set_portfolio(&mut svm, &trd_pf, &env.stub_perc, &env.market, &trd.pubkey(), 0, 0);

    register(&mut svm, &payer, &env, &ins, &ins.pubkey(), &ins_pos, COHORT_INSURANCE).expect("reg ins");
    register(&mut svm, &payer, &env, &back, &back.pubkey(), &back_pos, COHORT_BACKING).expect("reg back");
    register(&mut svm, &payer, &env, &lp, &lp.pubkey(), &lp_pf, COHORT_LP).expect("reg lp");
    register(&mut svm, &payer, &env, &trd, &trd.pubkey(), &trd_pf, COHORT_TRADER).expect("reg trd");

    // LP absorbs 10_000 residual_received; trader crystallizes 20_000 loss.
    set_slot(&mut svm, 1_500);
    set_portfolio(&mut svm, &lp_pf, &env.stub_perc, &env.market, &lp.pubkey(), 10_000, 0);
    set_portfolio(&mut svm, &trd_pf, &env.stub_perc, &env.market, &trd.pubkey(), 0, 20_000);
    crystallize(&mut svm, &payer, &env, &ins, &ins_pos).expect("cry ins");
    crystallize(&mut svm, &payer, &env, &back, &back_pos).expect("cry back");
    crystallize(&mut svm, &payer, &env, &lp, &lp_pf).expect("cry lp");
    crystallize(&mut svm, &payer, &env, &trd, &trd_pf).expect("cry trd");

    // Freeze after emission_end + finalize_window.
    set_slot(&mut svm, env.emission_end + env.finalize_window + 1);
    freeze(&mut svm, &payer, &env).expect("freeze");

    // Each cohort has a single staker -> claims the WHOLE cohort_supply (10/10/40/40 of 1_000_000).
    for (owner, linked, cohort, want, is_share) in [
        (&ins, &ins_pos, COHORT_INSURANCE, 100_000u64, true),
        (&back, &back_pos, COHORT_BACKING, 100_000u64, true),
        (&lp, &lp_pf, COHORT_LP, 400_000u64, false),
        (&trd, &trd_pf, COHORT_TRADER, 400_000u64, false),
    ] {
        let ata = create_token_account(&mut svm, &payer, &env.coin_mint, &owner.pubkey());
        claim(&mut svm, &payer, &env, &owner, &ata, if is_share { Some(linked) } else { None }).expect("claim");
        assert_eq!(token_amount(&svm, &ata), want, "cohort {cohort} payout");
    }
    // Conservation: total paid == supply (10+10+40+40 = 100%).
    assert_eq!(100_000 + 100_000 + 400_000 + 400_000, supply);
}

// SHARE VALUE is pro-rata by shares, and the soft veto: an insurance depositor who EXITS (shares -> 0
// at claim) forfeits its COIN even if it had crystallized points; the survivor still claims its own.
#[test]
fn share_value_is_pro_rata_and_exit_forfeits() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let supply = 1_000_000u64; // insurance cohort = 10% = 100_000
    let env = setup(&mut svm, &payer, supply);
    set_slot(&mut svm, 100);

    let (a, b) = (Keypair::new(), Keypair::new());
    let a_pos = Pubkey::new_unique();
    let b_pos = Pubkey::new_unique();
    set_position(&mut svm, &a_pos, &env.stub_sub, &env.ins_pool, &a.pubkey(), 300, false);
    set_position(&mut svm, &b_pos, &env.stub_sub, &env.ins_pool, &b.pubkey(), 100, false);
    register(&mut svm, &payer, &env, &a, &a.pubkey(), &a_pos, COHORT_INSURANCE).expect("reg a");
    register(&mut svm, &payer, &env, &b, &b.pubkey(), &b_pos, COHORT_INSURANCE).expect("reg b");
    crystallize(&mut svm, &payer, &env, &a, &a_pos).expect("cry a"); // 300 pts
    crystallize(&mut svm, &payer, &env, &b, &b_pos).expect("cry b"); // 100 pts (denom 400)

    // b EXITS before claim: redeems all shares -> withdrawn. (Denominator stays 400 -> b's 100 share burns.)
    set_position(&mut svm, &b_pos, &env.stub_sub, &env.ins_pool, &b.pubkey(), 0, true);

    set_slot(&mut svm, env.emission_end + env.finalize_window + 1);
    freeze(&mut svm, &payer, &env).expect("freeze");

    let a_ata = create_token_account(&mut svm, &payer, &env.coin_mint, &a.pubkey());
    let b_ata = create_token_account(&mut svm, &payer, &env.coin_mint, &b.pubkey());
    claim(&mut svm, &payer, &env, &a, &a_ata, Some(&a_pos)).expect("claim a");
    claim(&mut svm, &payer, &env, &b, &b_ata, Some(&b_pos)).expect("claim b");
    // a: 100_000 * 300/400 = 75_000. b: exited -> live shares 0 -> 0 (forfeit; its 25_000 stays in the vault).
    assert_eq!(token_amount(&svm, &a_ata), 75_000, "a pro-rata by shares");
    assert_eq!(token_amount(&svm, &b_ata), 0, "b exited -> soft-veto forfeit");
}

// finding KM: a share-value claim must be authorized by the stake's OWN owner. claim caps the payout by
// LIVE shares, so a permissionless trigger would let an attacker force the victim's claim during a
// transient low-share moment (mid partial-withdraw: withdrawn=false, shares reduced) and the irreversible
// claimed-flag would lock in the reduced payout. Here the attacker's forced claim is rejected, so the
// victim re-deposits and claims their FULL share themselves.
#[test]
fn share_value_claim_cannot_be_forced_by_a_third_party_at_a_low_share_moment() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let supply = 1_000_000u64; // insurance cohort = 10% = 100_000
    let env = setup(&mut svm, &payer, supply);
    set_slot(&mut svm, 100);

    let victim = Keypair::new();
    let attacker = Keypair::new();
    let pos = Pubkey::new_unique();
    set_position(&mut svm, &pos, &env.stub_sub, &env.ins_pool, &victim.pubkey(), 300, false);
    register(&mut svm, &payer, &env, &victim, &victim.pubkey(), &pos, COHORT_INSURANCE).expect("reg");
    crystallize(&mut svm, &payer, &env, &victim, &pos).expect("cry"); // 300 pts, denom 300
    set_slot(&mut svm, env.emission_end + env.finalize_window + 1);
    freeze(&mut svm, &payer, &env).expect("freeze");

    // victim is mid partial-withdraw: still a live backer (withdrawn=false) but shares transiently at 30.
    set_position(&mut svm, &pos, &env.stub_sub, &env.ins_pool, &victim.pubkey(), 30, false);
    let ata = create_token_account(&mut svm, &payer, &env.coin_mint, &victim.pubkey());
    // the attacker cannot force the victim's claim at the low-share moment.
    assert!(claim_as(&mut svm, &payer, &env, &attacker, &victim.pubkey(), &ata, Some(&pos)).is_err(),
        "a third party must not be able to force a share-value claim");
    assert_eq!(token_amount(&svm, &ata), 0, "nothing was paid out by the forced attempt");

    // the victim re-deposits to full shares and claims THEMSELVES -> full 100_000 (grief avoided).
    set_position(&mut svm, &pos, &env.stub_sub, &env.ins_pool, &victim.pubkey(), 300, false);
    claim(&mut svm, &payer, &env, &victim, &ata, Some(&pos)).expect("owner claims their own");
    assert_eq!(token_amount(&svm, &ata), 100_000, "victim claims their full pro-rata share");
}

// finding KO (KM parity, one step earlier): crystallize OVERWRITES a share-value stake's points from the
// live shares NOW, and freeze locks that as the frozen denominator term — which the claim-time min-cap can
// only lower, never raise. So a permissionless crystallize would let an attacker force a victim's points
// down at a transient low-share moment, then freeze to lock it. crystallize for share-value cohorts is
// therefore owner-gated. Here the attacker's forced crystallize is rejected; the owner re-crystallizes at
// full shares and claims their full share.
#[test]
fn share_value_crystallize_cannot_be_forced_by_a_third_party_at_a_low_share_moment() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let supply = 1_000_000u64; // insurance cohort = 10% = 100_000
    let env = setup(&mut svm, &payer, supply);
    set_slot(&mut svm, 100);

    let victim = Keypair::new();
    let attacker = Keypair::new();
    let pos = Pubkey::new_unique();
    set_position(&mut svm, &pos, &env.stub_sub, &env.ins_pool, &victim.pubkey(), 300, false);
    register(&mut svm, &payer, &env, &victim, &victim.pubkey(), &pos, COHORT_INSURANCE).expect("reg");
    crystallize(&mut svm, &payer, &env, &victim, &pos).expect("owner crystallizes at 300");

    // victim mid partial-withdraw -> live shares transiently 30. The attacker tries to force the points down.
    set_position(&mut svm, &pos, &env.stub_sub, &env.ins_pool, &victim.pubkey(), 30, false);
    assert!(crystallize_as(&mut svm, &payer, &env, &attacker, &victim.pubkey(), &pos).is_err(),
        "a third party must not be able to force a share-value crystallize");

    // victim restores shares and the genesis freezes; the victim claims their FULL 100_000 (grief avoided).
    set_position(&mut svm, &pos, &env.stub_sub, &env.ins_pool, &victim.pubkey(), 300, false);
    set_slot(&mut svm, env.emission_end + env.finalize_window + 1);
    freeze(&mut svm, &payer, &env).expect("freeze");
    let ata = create_token_account(&mut svm, &payer, &env.coin_mint, &victim.pubkey());
    claim(&mut svm, &payer, &env, &victim, &ata, Some(&pos)).expect("owner claims");
    assert_eq!(token_amount(&svm, &ata), 100_000, "victim's points were not force-lowered");
}

// REGISTER binds the linked account's owner (finding GY): a foreign signer cannot register against
// someone else's position/portfolio, and a position from a foreign pool is rejected (finding HG).
#[test]
fn register_rejects_foreign_owner_and_foreign_pool() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let env = setup(&mut svm, &payer, 1_000_000);
    set_slot(&mut svm, 100);

    let victim = Keypair::new();
    let attacker = Keypair::new();
    let pos = Pubkey::new_unique();
    set_position(&mut svm, &pos, &env.stub_sub, &env.ins_pool, &victim.pubkey(), 500, false);
    // attacker signs but the position.owner is the victim -> rejected.
    assert!(register(&mut svm, &payer, &env, &attacker, &attacker.pubkey(), &pos, COHORT_INSURANCE).is_err(),
        "foreign owner must be rejected");

    // a position in a FOREIGN pool (not the genesis insurance pool) -> rejected even for the owner.
    let foreign_pool = Pubkey::new_unique();
    let pos2 = Pubkey::new_unique();
    set_position(&mut svm, &pos2, &env.stub_sub, &foreign_pool, &victim.pubkey(), 500, false);
    assert!(register(&mut svm, &payer, &env, &victim, &victim.pubkey(), &pos2, COHORT_INSURANCE).is_err(),
        "foreign pool must be rejected");
}

// finding IL: the LP/trader cohorts must be scoped to the ONE allow-listed (trusted-Pyth) genesis
// market. An attacker who stands up their OWN percolator market with an oracle they control can
// wash-trade to mint crystallized_loss/received at will; here that portfolio belongs to a FOREIGN
// market_group, so register rejects it for both cohorts even though the attacker owns it and the
// counters are non-zero. The same attacker's portfolio in the genesis market would register fine.
#[test]
fn register_rejects_portfolio_from_a_foreign_market() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let env = setup(&mut svm, &payer, 1_000_000);
    set_slot(&mut svm, 100);

    let attacker = Keypair::new();
    let foreign_market = Pubkey::new_unique(); // attacker's own market, oracle they control
    // attacker owns the portfolio and has manufactured a fat loss/receipt — but in a foreign market.
    let evil_pf = Pubkey::new_unique();
    set_portfolio(&mut svm, &evil_pf, &env.stub_perc, &foreign_market, &attacker.pubkey(), 9_000_000, 9_000_000);
    assert!(register(&mut svm, &payer, &env, &attacker, &attacker.pubkey(), &evil_pf, COHORT_TRADER).is_err(),
        "trader cohort: a portfolio from a foreign (attacker-oracle'd) market must be rejected");
    assert!(register(&mut svm, &payer, &env, &attacker, &attacker.pubkey(), &evil_pf, COHORT_LP).is_err(),
        "lp cohort: a portfolio from a foreign market must be rejected");

    // the SAME attacker, but a portfolio in the genesis (allow-listed) market -> accepted.
    let good_pf = Pubkey::new_unique();
    set_portfolio(&mut svm, &good_pf, &env.stub_perc, &env.market, &attacker.pubkey(), 0, 0);
    register(&mut svm, &payer, &env, &attacker, &attacker.pubkey(), &good_pf, COHORT_TRADER)
        .expect("a portfolio in the allow-listed genesis market registers");
}

// LP/trader points are the Δ of the monotonic residual counter since register; claim is frozen-final
// (no live cap account), and double-claim is rejected.
#[test]
fn lp_residual_delta_and_double_claim_rejected() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let supply = 1_000_000u64; // lp cohort = 40% = 400_000
    let env = setup(&mut svm, &payer, supply);
    set_slot(&mut svm, 100);

    let lp = Keypair::new();
    let pf = Pubkey::new_unique();
    // register at received=5_000 (pre-existing); only the Δ after register should count.
    set_portfolio(&mut svm, &pf, &env.stub_perc, &env.market, &lp.pubkey(), 5_000, 0);
    register(&mut svm, &payer, &env, &lp, &lp.pubkey(), &pf, COHORT_LP).expect("reg lp");
    set_slot(&mut svm, 1_500);
    set_portfolio(&mut svm, &pf, &env.stub_perc, &env.market, &lp.pubkey(), 12_000, 0); // Δ = 7_000
    crystallize(&mut svm, &payer, &env, &lp, &pf).expect("cry lp");
    set_slot(&mut svm, env.emission_end + env.finalize_window + 1);
    freeze(&mut svm, &payer, &env).expect("freeze");

    let ata = create_token_account(&mut svm, &payer, &env.coin_mint, &lp.pubkey());
    claim(&mut svm, &payer, &env, &lp, &ata, None).expect("claim lp");
    // sole LP staker -> whole cohort supply regardless of the absolute Δ.
    assert_eq!(token_amount(&svm, &ata), 400_000, "sole LP claims the LP cohort supply");
    // double-claim rejected.
    assert!(claim(&mut svm, &payer, &env, &lp, &ata, None).is_err(), "double-claim must reject");
}

// claim is rejected before freeze (denominators not final).
#[test]
fn claim_before_freeze_is_rejected() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let env = setup(&mut svm, &payer, 1_000_000);
    set_slot(&mut svm, 100);
    let lp = Keypair::new();
    let pf = Pubkey::new_unique();
    set_portfolio(&mut svm, &pf, &env.stub_perc, &env.market, &lp.pubkey(), 0, 0);
    register(&mut svm, &payer, &env, &lp, &lp.pubkey(), &pf, COHORT_LP).expect("reg");
    set_portfolio(&mut svm, &pf, &env.stub_perc, &env.market, &lp.pubkey(), 1_000, 0);
    crystallize(&mut svm, &payer, &env, &lp, &pf).expect("cry");
    let ata = create_token_account(&mut svm, &payer, &env.coin_mint, &lp.pubkey());
    assert!(claim(&mut svm, &payer, &env, &lp, &ata, None).is_err(), "claim before freeze must reject");
}

// init validation guards: reject zero supply, a cohort bps sum > 100%, an active insurance/backing cohort
// with no pool scope, and an active LP/trader cohort with no market_group (finding IL — else an unscoped
// genesis could mint COIN to positions from any pool / any market). init reads only coin_mint.key (the mint
// is unpacked at freeze, not init), so a fresh random pubkey suffices as the coin_mint. Real .so.
fn try_init(svm: &mut LiteSVM, payer: &Keypair, supply: u64, ins: u16, back: u16, lp: u16, ins_pool: Pubkey, back_pool: Pubkey, market: Pubkey) -> Result<(), String> {
    let coin_mint = Pubkey::new_unique();
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8];
    d.extend_from_slice(&supply.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes());     // emission_end
    d.extend_from_slice(&ins.to_le_bytes());
    d.extend_from_slice(&back.to_le_bytes());
    d.extend_from_slice(&lp.to_le_bytes());
    d.extend_from_slice(&500u64.to_le_bytes());        // finalize_window
    d.extend_from_slice(ins_pool.as_ref());
    d.extend_from_slice(back_pool.as_ref());
    d.extend_from_slice(market.as_ref());
    send(svm, payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false),
        AccountMeta::new_readonly(dist_id(), false), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false), AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[])
}

#[test]
fn init_rejects_zero_supply_overallocation_and_unscoped_cohorts() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let p = Pubkey::new_unique;
    let z = Pubkey::default();

    // zero supply -> rejected.
    assert!(try_init(&mut svm, &payer, 0, 1_000, 1_000, 4_000, p(), p(), p()).is_err(), "zero supply");
    // cohort bps sum > 100% (5000+4000+2000=11000) -> rejected.
    assert!(try_init(&mut svm, &payer, 1_000_000, 5_000, 4_000, 2_000, p(), p(), p()).is_err(), "bps sum > 100%");
    // insurance active (10000, trader=0) but NO insurance pool -> rejected.
    assert!(try_init(&mut svm, &payer, 1_000_000, 10_000, 0, 0, z, z, z).is_err(), "insurance cohort without a pool scope");
    // backing active (10000, trader=0) but NO backing pool -> rejected.
    assert!(try_init(&mut svm, &payer, 1_000_000, 0, 10_000, 0, z, z, z).is_err(), "backing cohort without a pool scope");
    // LP+trader active (lp 4000, trader 4000) with pools set but NO market_group -> rejected (finding IL).
    assert!(try_init(&mut svm, &payer, 1_000_000, 1_000, 1_000, 4_000, p(), p(), z).is_err(), "lp/trader cohort without a market scope");
    // fully-valid config -> accepted.
    try_init(&mut svm, &payer, 1_000_000, 1_000, 1_000, 4_000, p(), p(), p()).expect("a fully-scoped config initializes");
}

// claim anti-theft (GY at the claim layer): LP/trader claim is PERMISSIONLESS (any cranker may finalize a
// backer's claim), so the cranker must NOT be able to (a) redirect the COIN to an account it controls, nor
// (b) pay from a decoy vault. The bound recipient + the config.vault are the only acceptable endpoints. Real .so.
#[test]
fn claim_cannot_be_redirected_or_paid_from_a_decoy_vault() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let env = setup(&mut svm, &payer, 1_000_000);
    set_slot(&mut svm, 100);

    let lp = Keypair::new();
    let pf = Pubkey::new_unique();
    set_portfolio(&mut svm, &pf, &env.stub_perc, &env.market, &lp.pubkey(), 0, 0);
    register(&mut svm, &payer, &env, &lp, &lp.pubkey(), &pf, COHORT_LP).expect("register"); // recipient bound = lp
    set_portfolio(&mut svm, &pf, &env.stub_perc, &env.market, &lp.pubkey(), 10_000, 0);
    crystallize(&mut svm, &payer, &env, &lp, &pf).expect("crystallize");
    set_slot(&mut svm, env.emission_end + env.finalize_window + 1);
    freeze(&mut svm, &payer, &env).expect("freeze");

    let attacker = Keypair::new();
    let stake = stake_pda(&env, &lp.pubkey());
    let rd_config = env.rd_config;
    let coin_mint = env.coin_mint;
    let real_vault = env.vault;
    let mut raw_claim = |svm: &mut LiteSVM, cranker: &Keypair, vault: Pubkey, recipient_ata: Pubkey| -> Result<(), String> {
        send(svm, &payer, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(cranker.pubkey(), true), AccountMeta::new_readonly(rd_config, false),
            AccountMeta::new(stake, false), AccountMeta::new(vault, false),
            AccountMeta::new(recipient_ata, false), AccountMeta::new_readonly(spl_token::ID, false),
        ], data: vec![5u8] }], &[cranker])
    };

    // (a) a third-party cranker redirecting to its OWN ata -> rejected (ra.owner != stake.recipient).
    let attacker_ata = create_token_account(&mut svm, &payer, &coin_mint, &attacker.pubkey());
    assert!(raw_claim(&mut svm, &attacker, real_vault, attacker_ata).is_err(),
        "claim cannot be redirected to a non-recipient ata");
    // (b) paying from a decoy vault -> rejected (vault.key != config.vault).
    let decoy_vault = create_token_account(&mut svm, &payer, &coin_mint, &attacker.pubkey());
    let lp_ata = create_token_account(&mut svm, &payer, &coin_mint, &lp.pubkey());
    assert!(raw_claim(&mut svm, &attacker, decoy_vault, lp_ata).is_err(),
        "claim cannot pay from a decoy vault");
    // (control) ANY cranker may finalize the claim, but ONLY into the bound recipient from the real vault.
    raw_claim(&mut svm, &attacker, real_vault, lp_ata).expect("permissionless cranker pays the bound recipient");
    assert!(token_amount(&svm, &lp_ata) > 0, "the LP backer received its share");
}

// register guards distinct from the foreign-owner/pool/market tests: an out-of-range cohort, CROSS-PROGRAM
// type confusion (a share-value cohort pointed at a percolator account, or an LP/trader cohort at a subledger
// position — the owner-PROGRAM check blocks reading the wrong struct at the bound offsets), and a
// double-register (the per-owner stake PDA already exists). Real .so.
#[test]
fn register_rejects_out_of_range_cohort_cross_program_and_double_register() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let env = setup(&mut svm, &payer, 1_000_000);
    set_slot(&mut svm, 100);
    let alice = Keypair::new();

    // (1) cohort 4 > COHORT_TRADER(3) -> rejected (the linked account isn't even read).
    let any = Pubkey::new_unique();
    assert!(register(&mut svm, &payer, &env, &alice, &alice.pubkey(), &any, 4).is_err(),
        "out-of-range cohort must reject");

    // (2) cross-program type confusion: insurance cohort pointed at a PERCOLATOR-owned account is rejected
    // (owner program != subledger_program), and symmetrically an LP cohort at a SUBLEDGER position.
    let perc_acct = Pubkey::new_unique();
    set_portfolio(&mut svm, &perc_acct, &env.stub_perc, &env.market, &alice.pubkey(), 5_000, 0);
    assert!(register(&mut svm, &payer, &env, &alice, &alice.pubkey(), &perc_acct, COHORT_INSURANCE).is_err(),
        "insurance cohort must reject a percolator-owned account (wrong program)");
    let sub_acct = Pubkey::new_unique();
    set_position(&mut svm, &sub_acct, &env.stub_sub, &env.ins_pool, &alice.pubkey(), 500, false);
    assert!(register(&mut svm, &payer, &env, &alice, &alice.pubkey(), &sub_acct, COHORT_LP).is_err(),
        "lp cohort must reject a subledger-owned position (wrong program)");

    // (3) double-register for the same owner (stake PDA now initialized) -> rejected.
    let pf = Pubkey::new_unique();
    set_portfolio(&mut svm, &pf, &env.stub_perc, &env.market, &alice.pubkey(), 0, 0);
    register(&mut svm, &payer, &env, &alice, &alice.pubkey(), &pf, COHORT_LP).expect("first register ok");
    assert!(register(&mut svm, &payer, &env, &alice, &alice.pubkey(), &pf, COHORT_LP).is_err(),
        "double-register (stake PDA already initialized) must reject");
}

// Self-service finalize lifecycle guards: freeze is rejected before emission_end+finalize_window (else a
// permissionless caller could freeze early and forfeit slow backers' un-crystallized points); after freeze,
// register and crystallize are closed (else the frozen denominator could be diluted/altered); and a
// double-freeze is rejected (snapshot + bound vault are immutable). Real .so.
#[test]
fn self_service_lifecycle_guards_freeze_window_and_post_freeze_closure() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let env = setup(&mut svm, &payer, 1_000_000);
    set_slot(&mut svm, 100);

    // an LP backer registers + crystallizes during the accrual phase.
    let lp = Keypair::new();
    let pf = Pubkey::new_unique();
    set_portfolio(&mut svm, &pf, &env.stub_perc, &env.market, &lp.pubkey(), 0, 0);
    register(&mut svm, &payer, &env, &lp, &lp.pubkey(), &pf, COHORT_LP).expect("register");
    set_portfolio(&mut svm, &pf, &env.stub_perc, &env.market, &lp.pubkey(), 5_000, 0);
    crystallize(&mut svm, &payer, &env, &lp, &pf).expect("crystallize");

    // (1) freeze BEFORE emission_end + finalize_window is rejected.
    set_slot(&mut svm, env.emission_end + env.finalize_window - 1);
    assert!(freeze(&mut svm, &payer, &env).is_err(), "freeze before the finalize window closes must reject");

    // window closes -> freeze succeeds (one-shot).
    set_slot(&mut svm, env.emission_end + env.finalize_window + 1);
    freeze(&mut svm, &payer, &env).expect("freeze after the window");

    // (2) register is closed after freeze (would dilute the frozen denominator).
    let late = Keypair::new();
    let late_pf = Pubkey::new_unique();
    set_portfolio(&mut svm, &late_pf, &env.stub_perc, &env.market, &late.pubkey(), 9_000, 0);
    assert!(register(&mut svm, &payer, &env, &late, &late.pubkey(), &late_pf, COHORT_LP).is_err(),
        "register after freeze must reject");

    // (3) crystallize is closed after freeze (would alter the frozen denominator).
    set_portfolio(&mut svm, &pf, &env.stub_perc, &env.market, &lp.pubkey(), 99_000, 0);
    assert!(crystallize(&mut svm, &payer, &env, &lp, &pf).is_err(), "crystallize after freeze must reject");

    // (4) double-freeze is rejected (snapshot + bound vault immutable).
    assert!(freeze(&mut svm, &payer, &env).is_err(), "double-freeze must reject");
}

// CONSERVATION property pin: across ALL FOUR cohorts with many stakes and deliberately NON-even point
// splits (so floor rounding leaves dust), the sum of claims must never exceed any cohort's supply nor the
// total supply, and the vault must be drained by EXACTLY the claimed total — never over-drawn. Real .so.
#[test]
fn cross_cohort_claims_never_exceed_cohort_or_total_supply() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let supply = 1_000_000u64; // ins 10% =100k, back 10% =100k, lp 40% =400k, trader 40% =400k
    let env = setup(&mut svm, &payer, supply);
    set_slot(&mut svm, 100);

    // Deliberately non-dividing denominators -> floor dust (Σ < cohort_supply) for several cohorts.
    let ins: Vec<(Keypair, Pubkey, u128)> =
        vec![(Keypair::new(), Pubkey::new_unique(), 1), (Keypair::new(), Pubkey::new_unique(), 1), (Keypair::new(), Pubkey::new_unique(), 1)];
    let back: Vec<(Keypair, Pubkey, u128)> =
        vec![(Keypair::new(), Pubkey::new_unique(), 200), (Keypair::new(), Pubkey::new_unique(), 800)];
    let lp: Vec<(Keypair, Pubkey, u128)> =
        vec![(Keypair::new(), Pubkey::new_unique(), 1_000), (Keypair::new(), Pubkey::new_unique(), 3_000), (Keypair::new(), Pubkey::new_unique(), 7)];
    let trd: Vec<(Keypair, Pubkey, u128)> =
        vec![(Keypair::new(), Pubkey::new_unique(), 333), (Keypair::new(), Pubkey::new_unique(), 333), (Keypair::new(), Pubkey::new_unique(), 334)];

    for (o, pos, shares) in &ins {
        set_position(&mut svm, pos, &env.stub_sub, &env.ins_pool, &o.pubkey(), *shares, false);
        register(&mut svm, &payer, &env, o, &o.pubkey(), pos, COHORT_INSURANCE).unwrap();
        crystallize(&mut svm, &payer, &env, o, pos).unwrap();
    }
    for (o, pos, shares) in &back {
        set_position(&mut svm, pos, &env.stub_sub, &env.back_pool, &o.pubkey(), *shares, false);
        register(&mut svm, &payer, &env, o, &o.pubkey(), pos, COHORT_BACKING).unwrap();
        crystallize(&mut svm, &payer, &env, o, pos).unwrap();
    }
    for (o, pf, recv) in &lp {
        set_portfolio(&mut svm, pf, &env.stub_perc, &env.market, &o.pubkey(), *recv, 0);
        register(&mut svm, &payer, &env, o, &o.pubkey(), pf, COHORT_LP).unwrap();
        crystallize(&mut svm, &payer, &env, o, pf).unwrap();
    }
    for (o, pf, cryst) in &trd {
        set_portfolio(&mut svm, pf, &env.stub_perc, &env.market, &o.pubkey(), 0, *cryst);
        register(&mut svm, &payer, &env, o, &o.pubkey(), pf, COHORT_TRADER).unwrap();
        crystallize(&mut svm, &payer, &env, o, pf).unwrap();
    }

    set_slot(&mut svm, env.emission_end + env.finalize_window + 1);
    freeze(&mut svm, &payer, &env).unwrap();

    let mut claim_cohort = |svm: &mut LiteSVM, members: &[(Keypair, Pubkey, u128)], share_value: bool| -> u64 {
        let mut sum = 0u64;
        for (o, linked, _) in members {
            let ata = create_token_account(svm, &payer, &env.coin_mint, &o.pubkey());
            claim(svm, &payer, &env, o, &ata, if share_value { Some(linked) } else { None }).expect("claim");
            sum += token_amount(svm, &ata);
        }
        sum
    };
    let ins_sum = claim_cohort(&mut svm, &ins, true);
    let back_sum = claim_cohort(&mut svm, &back, true);
    let lp_sum = claim_cohort(&mut svm, &lp, false);
    let trd_sum = claim_cohort(&mut svm, &trd, false);

    let cs = |bps: u128| (supply as u128 * bps / 10_000) as u64;
    assert!(ins_sum <= cs(1_000), "insurance Σ <= cohort supply");
    assert!(back_sum <= cs(1_000), "backing Σ <= cohort supply");
    assert!(lp_sum <= cs(4_000), "lp Σ <= cohort supply");
    assert!(trd_sum <= cs(4_000), "trader Σ <= cohort supply");
    let total = ins_sum + back_sum + lp_sum + trd_sum;
    assert!(total <= supply, "total claims never exceed the fixed supply");
    assert_eq!(token_amount(&svm, &env.vault), supply - total, "vault drained by EXACTLY the claimed total — never over");
    // the non-even insurance split (3 equal shares, denom 3) must leave floor dust, proving Σ < cohort_supply.
    assert!(ins_sum < cs(1_000), "floor rounding leaves dust: Σ strictly under cohort supply");
}

// --- freeze GX/EZ guards (previously only the happy path was exercised; the src comment even cited a
// `set_authority_clears_delegate_no_vault_rug` test that never existed). These pin the negatives. ---
fn create_mint_with_freeze(svm: &mut LiteSVM, payer: &Keypair, mint_auth: &Pubkey, freeze_auth: Option<&Pubkey>) -> Pubkey {
    let mint = Keypair::new();
    let rent = svm.minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN);
    let ixs = [
        system_instruction::create_account(&payer.pubkey(), &mint.pubkey(), rent, spl_token::state::Mint::LEN as u64, &spl_token::ID),
        spl_token::instruction::initialize_mint(&spl_token::ID, &mint.pubkey(), mint_auth, freeze_auth, 6).unwrap(),
    ];
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[payer, &mint], svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();
    mint.pubkey()
}
// Init an rd_config for a prepared coin_mint (the vault is bound later at freeze). emission_end=2000, window=500.
fn rd_init(svm: &mut LiteSVM, payer: &Keypair, supply: u64, coin_mint: &Pubkey) -> Pubkey {
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), rd_config.as_ref()], &dist_id()).0;
    let mut d = vec![0u8];
    d.extend_from_slice(&supply.to_le_bytes());
    d.extend_from_slice(&2_000u64.to_le_bytes()); // emission_end
    d.extend_from_slice(&1_000u16.to_le_bytes()); // insurance
    d.extend_from_slice(&1_000u16.to_le_bytes()); // backing
    d.extend_from_slice(&4_000u16.to_le_bytes()); // lp (trader remainder)
    d.extend_from_slice(&500u64.to_le_bytes());   // finalize_window
    d.extend_from_slice(Pubkey::new_unique().as_ref()); // ins_pool
    d.extend_from_slice(Pubkey::new_unique().as_ref()); // back_pool
    d.extend_from_slice(Pubkey::new_unique().as_ref()); // market
    send(svm, payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(*coin_mint, false),
        AccountMeta::new_readonly(dist_id(), false), AccountMeta::new_readonly(dist_config, false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false), AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
    ], data: d }], &[]).expect("rd init");
    rd_config
}
fn freeze_ix(svm: &mut LiteSVM, payer: &Keypair, rd_config: Pubkey, coin_mint: Pubkey, vault: Pubkey) -> Result<(), String> {
    send(svm, payer, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false),
        AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(vault, false),
    ], data: vec![4u8] }], &[])
}

// finding GX/EZ: freeze BINDS the fixed-supply COIN vault, so it must reject any mint that could still be
// inflated (live mint authority) or freeze claimers (live freeze authority), and any vault that isn't the
// rd_config-owned full-supply account. Each case uses its own mint so the global mint.supply check isolates
// the guard under test.
#[test]
fn freeze_enforces_fixed_supply_and_vault_integrity() {
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(rd_id(), rd_so()).unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
    let supply = 1_000_000u64;
    let past = 2_000u64 + 500 + 1;

    // (GX) a mint that still has a MINT authority is rejected (supply could be inflated under claimers);
    // after revoking it, the fixed-supply mint + rd-owned full vault is accepted.
    let ma = Keypair::new();
    let mint = create_mint(&mut svm, &payer, &ma.pubkey());
    let rd_config = rd_init(&mut svm, &payer, supply, &mint);
    let vault = create_token_account(&mut svm, &payer, &mint, &rd_config);
    mint_to(&mut svm, &payer, &mint, &ma, &vault, supply);
    set_slot(&mut svm, past);
    assert!(freeze_ix(&mut svm, &payer, rd_config, mint, vault).is_err(), "live mint authority must be rejected (GX inflation)");
    revoke_mint(&mut svm, &payer, &mint, &ma);
    assert!(freeze_ix(&mut svm, &payer, rd_config, mint, vault).is_ok(), "fixed-supply mint + rd-owned full vault accepted");

    // (GX) a mint that still has a FREEZE authority is rejected (claimers' COIN could be frozen = censorship);
    // after clearing it, accepted.
    let ma2 = Keypair::new();
    let fa = Keypair::new();
    let mint2 = create_mint_with_freeze(&mut svm, &payer, &ma2.pubkey(), Some(&fa.pubkey()));
    let rd2 = rd_init(&mut svm, &payer, supply, &mint2);
    let vault2 = create_token_account(&mut svm, &payer, &mint2, &rd2);
    mint_to(&mut svm, &payer, &mint2, &ma2, &vault2, supply);
    revoke_mint(&mut svm, &payer, &mint2, &ma2); // clear mint authority; freeze authority remains
    set_slot(&mut svm, past);
    assert!(freeze_ix(&mut svm, &payer, rd2, mint2, vault2).is_err(), "live freeze authority must be rejected (GX freeze-claimers)");
    let clear = spl_token::instruction::set_authority(&spl_token::ID, &mint2, None, AuthorityType::FreezeAccount, &fa.pubkey(), &[]).unwrap();
    send(&mut svm, &payer, &[clear], &[&fa]).expect("clear freeze authority");
    assert!(freeze_ix(&mut svm, &payer, rd2, mint2, vault2).is_ok(), "after clearing freeze authority, accepted");

    // (EZ) a vault NOT owned by rd_config is rejected even when fully funded.
    let ma3 = Keypair::new();
    let mint3 = create_mint(&mut svm, &payer, &ma3.pubkey());
    let rd3 = rd_init(&mut svm, &payer, supply, &mint3);
    let attacker = Pubkey::new_unique();
    let decoy = create_token_account(&mut svm, &payer, &mint3, &attacker);
    mint_to(&mut svm, &payer, &mint3, &ma3, &decoy, supply);
    revoke_mint(&mut svm, &payer, &mint3, &ma3);
    set_slot(&mut svm, past);
    assert!(freeze_ix(&mut svm, &payer, rd3, mint3, decoy).is_err(), "non-rd-owned vault must be rejected (EZ)");

    // (EZ) an rd-owned but UNDER-funded vault is rejected (mint.supply == total, but the bound vault holds < it).
    let ma4 = Keypair::new();
    let mint4 = create_mint(&mut svm, &payer, &ma4.pubkey());
    let rd4 = rd_init(&mut svm, &payer, supply, &mint4);
    let under = create_token_account(&mut svm, &payer, &mint4, &rd4);
    let sink = create_token_account(&mut svm, &payer, &mint4, &Pubkey::new_unique());
    mint_to(&mut svm, &payer, &mint4, &ma4, &under, supply - 1);
    mint_to(&mut svm, &payer, &mint4, &ma4, &sink, 1); // total minted == supply, but `under` holds supply-1
    revoke_mint(&mut svm, &payer, &mint4, &ma4);
    set_slot(&mut svm, past);
    assert!(freeze_ix(&mut svm, &payer, rd4, mint4, under).is_err(), "under-funded rd-owned vault must be rejected (EZ)");
}
