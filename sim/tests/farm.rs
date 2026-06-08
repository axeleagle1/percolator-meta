//! Adversarial farming simulation — how a RATIONAL MINER maximizes its share of the deterministic
//! distributor (residual-distributor) across N markets whose ORACLE IT CANNOT CONTROL.
//!
//! Threat model. The N markets are trusted-Pyth (all allow-listed in the rd config). A NEUTRAL oracle key
//! (not the miner) moves the marks; the miner can never push them. The miner farms the two PnL-flow cohorts
//!   - TRADER  (points = Δ residual_crystallized_loss)
//!   - LP      (points = Δ residual_received)
//! by holding BOTH sides of a position per market: a long in one portfolio and a short in another, both
//! miner-owned (delta-neutral -> ZERO market risk). Whichever way the neutral oracle moves, the losing leg's
//! settled loss becomes crystallized_loss (trader points) and the winning leg's gain becomes received (LP
//! points) — both captured by the miner. The "loss" is an internal transfer between two accounts the miner
//! owns, so the miner's net capital is preserved; the only cost is trading fees. NOTE: those fees are NOT zero
//! even when a trade passes fee_bps=0 — percolator charges max(caller_fee_bps, trade_fee_base_bps), so the 3 bps
//! market base applies to every trade and accrues to asset-0 insurance (see the fee-accrual test below). That
//! only RAISES the miner's real cost, so the wash-unprofitability conclusion holds a fortiori.
//!
//! Result: even with markets it cannot oracle-control, the miner captures ~the ENTIRE LP+trader allocation
//! (80% of supply by default). The allow-list (finding IL+) closes the risk-free oracle-controlled path but
//! NOT the delta-neutral wash-farm. Run: RUST_MIN_STACK=8388608 cargo test -p sim -- --nocapture

use litesvm::LiteSVM;
use solana_sdk::{
    account::Account, clock::Clock, instruction::{AccountMeta, Instruction}, pubkey::Pubkey,
    signature::{Keypair, Signer}, system_program, transaction::Transaction,
};
use std::str::FromStr;
use percolator_prog::ix::Instruction as PIx;

const N_MARKETS: usize = 10;   // markets the miner CANNOT control the oracle of
const SUPPLY: u64 = 1_000_000; // fixed COIN supply
// cohorts: insurance 10 / backing 10 / lp 40 / trader 40 -> LP+trader = 80% is the farmable surface.
const INS_BPS: u16 = 1_000;
const BACK_BPS: u16 = 1_000;
const LP_BPS: u16 = 4_000; // trader = remainder = 4_000
const FEE_BPS: u16 = 2_000; // anti-wash fee (finding NZ) skimmed from LP/trader claims

fn perc_id() -> Pubkey { percolator_prog::id() }
fn perc_so() -> String { format!("{}/../../percolator-prog/target/deploy/percolator_prog.so", env!("CARGO_MANIFEST_DIR")) }
fn so_deploy(name: &str) -> String { format!("{}/../target/deploy/{}.so", env!("CARGO_MANIFEST_DIR"), name) }
fn rd_id() -> Pubkey { Pubkey::from_str("Res1dua1Distr1butor111111111111111111111111").unwrap() }
fn dist_id() -> Pubkey { Pubkey::from_str("D1str1but1on11111111111111111111111111111111").unwrap() }
fn sub_id() -> Pubkey { Pubkey::from_str("Sub1edger1111111111111111111111111111111111").unwrap() }
const ATA_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

fn token_acct_bytes(mint: &Pubkey, owner: &Pubkey, amount: u64) -> Vec<u8> {
    let mut d = vec![0u8; 165];
    d[0..32].copy_from_slice(mint.as_ref());
    d[32..64].copy_from_slice(owner.as_ref());
    d[64..72].copy_from_slice(&amount.to_le_bytes());
    d[108] = 1;
    d
}
fn set_token(svm: &mut LiteSVM, key: &Pubkey, mint: &Pubkey, owner: &Pubkey, amount: u64) {
    svm.set_account(*key, Account { lamports: 2_000_000, data: token_acct_bytes(mint, owner, amount), owner: spl_token::ID, executable: false, rent_epoch: 0 }).unwrap();
}
fn token_amount(svm: &LiteSVM, key: &Pubkey) -> u64 {
    svm.get_account(key).map(|a| u64::from_le_bytes(a.data[64..72].try_into().unwrap())).unwrap_or(0)
}
fn create_real_mint(svm: &mut LiteSVM, payer: &Keypair, authority: &Pubkey) -> Pubkey {
    let mint = Keypair::new();
    let rent = svm.minimum_balance_for_rent_exemption(82);
    let ixs = [
        solana_sdk::system_instruction::create_account(&payer.pubkey(), &mint.pubkey(), rent, 82, &spl_token::ID),
        spl_token::instruction::initialize_mint(&spl_token::ID, &mint.pubkey(), authority, None, 6).unwrap(),
    ];
    let bh = svm.latest_blockhash();
    svm.send_transaction(Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[payer, &mint], bh)).unwrap();
    mint.pubkey()
}
fn pix(accounts: Vec<AccountMeta>, ix: PIx) -> Instruction { Instruction { program_id: perc_id(), accounts, data: ix.encode() } }
fn read_u128_at(svm: &LiteSVM, pf: &Pubkey, off: usize) -> u128 {
    let d = svm.get_account(pf).unwrap().data;
    u128::from_le_bytes(d[off..off + 16].try_into().unwrap())
}
fn read_crystallized(svm: &LiteSVM, pf: &Pubkey) -> u128 { read_u128_at(svm, pf, 196) } // HEADER_LEN(16)+180
fn read_spent(svm: &LiteSVM, pf: &Pubkey) -> u128 { read_u128_at(svm, pf, 212) }        // HEADER_LEN(16)+196
fn read_received(svm: &LiteSVM, pf: &Pubkey) -> u128 { read_u128_at(svm, pf, 228) }     // HEADER_LEN(16)+212
fn perc_vault_authority(slab: &Pubkey) -> Pubkey { Pubkey::find_program_address(&[b"vault", slab.as_ref()], &perc_id()).0 }
fn canonical_insurance_vault(va: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[va.as_ref(), spl_token::ID.as_ref(), mint.as_ref()], &ATA_PROGRAM_ID).0
}
// Asset-0 domain insurance budget, read straight from the market slab (offset 448+301=749, u128).
// This is the figure the twap surplus pull reads (subledger PERC_INSURANCE_OFFSET / twap INSURANCE_OFFSET).
fn read_insurance(svm: &LiteSVM, market: &Pubkey) -> u128 {
    let acc = svm.get_account(market).unwrap();
    u128::from_le_bytes(acc.data[749..765].try_into().unwrap())
}
fn dist_config_pda(coin_mint: &Pubkey, authority: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"dist_config", coin_mint.as_ref(), authority.as_ref()], &dist_id()).0
}

#[test]
fn rational_miner_farms_the_deterministic_distributor_across_uncontrolled_markets() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    svm.add_program_from_file(rd_id(), so_deploy("residual_distributor")).unwrap();
    let payer = Keypair::new(); svm.airdrop(&payer.pubkey(), 100_000_000_000_000).unwrap();
    // NEUTRAL oracle authority — NOT the miner. The miner cannot push these marks.
    let oracle = Keypair::new(); svm.airdrop(&oracle.pubkey(), 1_000_000_000_000).unwrap();
    svm.set_sysvar(&Clock { slot: 100, unix_timestamp: 100, ..Default::default() });
    let collateral = create_real_mint(&mut svm, &payer, &Keypair::new().pubkey());
    let initial_price = 1_000u64;

    let send = |svm: &mut LiteSVM, ixs: &[Instruction], extra: &[&Keypair]| {
        svm.expire_blockhash();
        let bh = svm.latest_blockhash();
        let mut s: Vec<&Keypair> = vec![&payer]; s.extend_from_slice(extra);
        svm.send_transaction(Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), &s, bh))
    };

    // ---- stand up N trusted-Pyth markets (oracle moved ONLY by the neutral `oracle` key) ----
    let mlen = percolator_prog::state::market_account_len_for_capacity(1).unwrap();
    let mut markets: Vec<(Pubkey, Pubkey)> = Vec::new(); // (market, perc_vault)
    for _ in 0..N_MARKETS {
        let market = Pubkey::new_unique();
        svm.set_account(market, Account { lamports: 1_000_000_000, data: vec![0u8; mlen], owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
        let pv = canonical_insurance_vault(&perc_vault_authority(&market), &collateral);
        set_token(&mut svm, &pv, &collateral, &perc_vault_authority(&market), 0);
        send(&mut svm, &[pix(
            vec![AccountMeta::new(oracle.pubkey(), true), AccountMeta::new(market, false), AccountMeta::new_readonly(collateral, false)],
            PIx::InitMarket { max_portfolio_assets: 1, h_min: 0, h_max: 10, initial_price,
                min_nonzero_mm_req: 1, min_nonzero_im_req: 2, maintenance_margin_bps: 10_000, initial_margin_bps: 10_000,
                max_trading_fee_bps: 10_000, trade_fee_base_bps: 3, liquidation_fee_bps: 0, liquidation_fee_cap: 0,
                min_liquidation_abs: 0, max_price_move_bps_per_slot: 10_000, max_accrual_dt_slots: 1,
                max_abs_funding_e9_per_slot: 0, min_funding_lifetime_slots: 1, max_account_b_settlement_chunks: 1,
                max_bankrupt_close_chunks: 1, max_bankrupt_close_lifetime_slots: 100, public_b_chunk_atoms: percolator::MAX_VAULT_TVL,
                maintenance_fee_per_slot: 0 },
        )], &[&oracle]).expect("init market");
        send(&mut svm, &[pix(
            vec![AccountMeta::new(oracle.pubkey(), true), AccountMeta::new(market, false)],
            PIx::ConfigureAuthMark { asset_index: 0, now_slot: 100, initial_mark_e6: initial_price },
        )], &[&oracle]).expect("configure auth mark");
        markets.push((market, pv));
    }

    // ---- residual-distributor: cohorts 10/10/40/40, allow-list = ALL N markets ----
    let coin_auth = Keypair::new();
    let coin_mint = create_real_mint(&mut svm, &payer, &coin_auth.pubkey());
    let rd_config = Pubkey::find_program_address(&[b"rd_config", coin_mint.as_ref()], &rd_id()).0;
    let dist_config = dist_config_pda(&coin_mint, &rd_config);
    let rd_vault = Pubkey::new_unique(); set_token(&mut svm, &rd_vault, &coin_mint, &rd_config, 0);
    send(&mut svm, &[
        spl_token::instruction::mint_to(&spl_token::ID, &coin_mint, &rd_vault, &coin_auth.pubkey(), &[], SUPPLY).unwrap(),
        spl_token::instruction::set_authority(&spl_token::ID, &coin_mint, None, spl_token::instruction::AuthorityType::MintTokens, &coin_auth.pubkey(), &[]).unwrap(),
    ], &[&coin_auth]).expect("fund + freeze the COIN supply");
    let mut ri = vec![0u8];
    ri.extend_from_slice(&SUPPLY.to_le_bytes()); ri.extend_from_slice(&2_000u64.to_le_bytes());
    ri.extend_from_slice(&INS_BPS.to_le_bytes()); ri.extend_from_slice(&BACK_BPS.to_le_bytes()); ri.extend_from_slice(&LP_BPS.to_le_bytes());
    ri.extend_from_slice(&100u64.to_le_bytes());
    // insurance/backing pools: placeholders (no ins/back participants here; the farmable surface is the LP+trader 80%).
    ri.extend_from_slice(Pubkey::new_unique().as_ref()); ri.extend_from_slice(Pubkey::new_unique().as_ref());
    ri.extend_from_slice(markets[0].0.as_ref());           // market_group (primary)
    ri.push((N_MARKETS - 1) as u8);                        // extra allow-listed markets
    for (m, _) in &markets[1..] { ri.extend_from_slice(m.as_ref()); }
    ri.extend_from_slice(&FEE_BPS.to_le_bytes());          // OPTIONAL trailing anti-wash fee (finding NZ)
    send(&mut svm, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new_readonly(dist_id(), false),
        AccountMeta::new_readonly(dist_config, false), AccountMeta::new_readonly(perc_id(), false), AccountMeta::new_readonly(sub_id(), false),
        AccountMeta::new(rd_config, false), AccountMeta::new_readonly(system_program::ID, false),
    ], data: ri }], &[]).expect("rd init with the N-market allow-list");

    // ---- THE FARM: per market, a delta-neutral miner pair (long + short, both miner-owned) ----
    let plen = percolator_prog::state::portfolio_account_len_for_market_slots(2).unwrap();
    let posq = (percolator::POS_SCALE / 2) as i128;
    // (owner, cohort, portfolio, coin_ata)
    let mut stakes: Vec<(Keypair, u8, Pubkey, Pubkey)> = Vec::new();
    for (market, pv) in &markets {
        let long = Keypair::new(); let short = Keypair::new();
        svm.airdrop(&long.pubkey(), 1_000_000_000).unwrap(); svm.airdrop(&short.pubkey(), 1_000_000_000).unwrap();
        let long_pf = Pubkey::new_unique(); let short_pf = Pubkey::new_unique();
        for (o, pf) in [(&short, &short_pf), (&long, &long_pf)] {
            svm.set_account(*pf, Account { lamports: 1_000_000_000, data: vec![0u8; plen], owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
            send(&mut svm, &[pix(vec![AccountMeta::new(o.pubkey(), true), AccountMeta::new(*market, false), AccountMeta::new(*pf, false)], PIx::InitPortfolio)], &[o]).expect("init pf");
            let src = Pubkey::new_unique(); set_token(&mut svm, &src, &collateral, &o.pubkey(), 1_000_000);
            send(&mut svm, &[pix(vec![
                AccountMeta::new(o.pubkey(), true), AccountMeta::new(*market, false), AccountMeta::new(*pf, false),
                AccountMeta::new(src, false), AccountMeta::new(*pv, false), AccountMeta::new_readonly(spl_token::ID, false)],
                PIx::Deposit { amount: 1_000_000 })], &[o]).expect("deposit");
        }
        // register BEFORE the loss (snapshot = 0): long -> TRADER, short -> LP.
        for (o, cohort, pf) in [(&long, 3u8, long_pf), (&short, 2u8, short_pf)] {
            let stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), o.pubkey().as_ref()], &rd_id()).0;
            send(&mut svm, &[Instruction { program_id: rd_id(), accounts: vec![
                AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new_readonly(o.pubkey(), true),
                AccountMeta::new_readonly(o.pubkey(), false), AccountMeta::new_readonly(pf, false), AccountMeta::new(stake, false),
                AccountMeta::new_readonly(system_program::ID, false),
            ], data: vec![1u8, cohort] }], &[o]).expect("register");
            let ata = Pubkey::new_unique(); set_token(&mut svm, &ata, &coin_mint, &o.pubkey(), 0);
            stakes.push((o.insecure_clone(), cohort, pf, ata));
        }
        // open the delta-neutral pair: size_q negative -> owner_a(short) short, owner_b(long) long.
        send(&mut svm, &[pix(vec![
            AccountMeta::new(short.pubkey(), true), AccountMeta::new(long.pubkey(), true), AccountMeta::new(*market, false),
            AccountMeta::new(short_pf, false), AccountMeta::new(long_pf, false)],
            PIx::TradeNoCpi { asset_index: 0, size_q: -posq, exec_price: initial_price, fee_bps: 0 })], &[&short, &long]).expect("open delta-neutral pair");
    }

    // ---- the NEUTRAL oracle moves each market (miner does NOT sign), then a permissionless crank
    //      crystallizes the long's loss (trader) and the short's gain (received -> LP) ----
    svm.set_sysvar(&Clock { slot: 110, unix_timestamp: 110, ..Default::default() });
    let crank = |svm: &mut LiteSVM, market: &Pubkey, pf: &Pubkey, action: u8| {
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        svm.send_transaction(Transaction::new_signed_with_payer(&[pix(
            vec![AccountMeta::new(payer.pubkey(), true), AccountMeta::new(*market, false), AccountMeta::new(*pf, false)],
            PIx::PermissionlessCrank { action, asset_index: 0, now_slot: 110, funding_rate_e9: 0, close_q: 0, fee_bps: 0, recovery_reason: 0 },
        )], Some(&payer.pubkey()), &[&payer], bh))
    };
    let mut market_cryst = 0u128; let mut market_recv = 0u128; let mut market_spent = 0u128;
    for (i, (market, _)) in markets.iter().enumerate() {
        // the two portfolios of market i are stakes[2i] (long/trader) and stakes[2i+1] (short/lp).
        let long_pf = stakes[2 * i].2; let short_pf = stakes[2 * i + 1].2;
        send(&mut svm, &[pix(vec![AccountMeta::new(oracle.pubkey(), true), AccountMeta::new(*market, false)],
            PIx::PushAuthMark { asset_index: 0, now_slot: 110, mark_e6: initial_price / 2 })], &[&oracle]).expect("neutral oracle moves the mark");
        for pf in [&short_pf, &long_pf] { crank(&mut svm, market, pf, 2).expect("settle B"); crank(&mut svm, market, pf, 0).expect("refresh"); }
        market_cryst += read_crystallized(&svm, &long_pf);
        market_spent += read_spent(&svm, &long_pf);     // finding NZ: stays 0 -> spent-netting does NOT catch the delta-neutral wash
        market_recv += read_received(&svm, &short_pf);
    }

    // ---- crystallize every miner stake (Δ), freeze, claim ----
    for (o, _cohort, pf, _ata) in &stakes {
        let stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), o.pubkey().as_ref()], &rd_id()).0;
        send(&mut svm, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new(stake, false), AccountMeta::new_readonly(*pf, false),
        ], data: vec![2u8] }], &[]).expect("crystallize");
    }
    svm.set_sysvar(&Clock { slot: 2_101, unix_timestamp: 2_101, ..Default::default() });
    send(&mut svm, &[Instruction { program_id: rd_id(), accounts: vec![
        AccountMeta::new(payer.pubkey(), true), AccountMeta::new(rd_config, false), AccountMeta::new_readonly(coin_mint, false), AccountMeta::new(rd_vault, false),
    ], data: vec![4u8] }], &[]).expect("freeze");
    let mut miner_coin = 0u64;
    for (o, _cohort, _pf, ata) in &stakes {
        let stake = Pubkey::find_program_address(&[b"rd_stake", rd_config.as_ref(), o.pubkey().as_ref()], &rd_id()).0;
        send(&mut svm, &[Instruction { program_id: rd_id(), accounts: vec![
            AccountMeta::new(payer.pubkey(), true), AccountMeta::new_readonly(rd_config, false), AccountMeta::new(stake, false),
            AccountMeta::new(rd_vault, false), AccountMeta::new(*ata, false), AccountMeta::new_readonly(spl_token::ID, false),
        ], data: vec![5u8] }], &[]).expect("claim");
        miner_coin += token_amount(&svm, ata);
    }

    let trader_bps = 10_000 - INS_BPS - BACK_BPS - LP_BPS;
    let trader_supply = SUPPLY as u128 * trader_bps as u128 / 10_000;
    let lp_trader_supply = SUPPLY as u128 * (LP_BPS + trader_bps) as u128 / 10_000;
    println!("\n================ DETERMINISTIC DISTRIBUTOR FARMING SIM ================");
    println!("markets the miner CANNOT oracle-control : {N_MARKETS}  (trusted-Pyth, all allow-listed)");
    println!("cohorts                                 : insurance {}% / backing {}% / lp {}% / trader {}%",
        INS_BPS / 100, BACK_BPS / 100, LP_BPS / 100, trader_bps / 100);
    println!("strategy                                : delta-neutral wash — long+short per market, both miner-owned");
    println!("manufactured trader points (Σ crystallized loss) : {market_cryst}");
    println!("Σ spent (counterparty recovery of that loss)     : {market_spent}   <-- 0: the wash drains the backstop,");
    println!("    so trader points = crystallized - spent = crystallized: the spent-NETTING fix does NOT catch a");
    println!("    delta-neutral wash (the offsetting short gain lives in `pnl`, not in any residual counter).");
    println!("anti-wash fee on LP/trader claims        : {}%  (finding NZ)", FEE_BPS / 100);
    println!("COIN captured (TRADER cohort, post-fee)  : {miner_coin}  of {SUPPLY}  ({:.1}% of total supply)", miner_coin as f64 * 100.0 / SUPPLY as f64);
    println!("                                        : {:.0}% of the TRADER cohort ({trader_supply}) — the fee skimmed the other {}%", miner_coin as f64 * 100.0 / trader_supply as f64, FEE_BPS / 100);
    println!("market risk taken                       : ZERO (delta-neutral; long+short both miner-owned)");
    println!("-----------------------------------------------------------------------");
    println!("VERDICT: the wash is NOT structurally closeable by counters (spent stays 0). The fee TAXES it:");
    println!("  net miner take = cohort * (1 - fee) - trading_fees - locked-margin opportunity cost. The fee +");
    println!("  the capital the miner must lock in the delta-neutral pairs (the Sybil cost) are the real bound;");
    println!("  a hard per-participant cap is the stronger lever if a guaranteed bound is wanted. The LP cohort");
    println!("  (Δ received, {}%) is farmable the same way and taxed the same way.", LP_BPS / 100);
    println!("=======================================================================\n");

    // ---- 9 NORMAL traders vs 1 FARMER (dilution under anonymity) ----
    // The 80% above is the SYBIL case: ONE entity owning all the trader stakes. But each trader stake is a
    // distinct owner, so a LONE farmer among N independent participants captures only its OWN stake = ~1/N of
    // the cohort. Here N = the number of trader stakes; a single farmer's take is one of them. The rd CANNOT
    // tell a delta-neutral farmer's wash-loss from a normal directional trader's REAL loss (both are just
    // crystallized loss), so 9 normal traders' real backing dilutes the 1 farmer to 1/10 — and to beat the
    // dilution the farmer must Sybil into more accounts, each needing its OWN locked capital + per-trade fee.
    let trader_stakes: Vec<u64> = stakes.iter().filter(|(_, c, _, _)| *c == 3).map(|(_, _, _, a)| token_amount(&svm, a)).collect();
    let n = trader_stakes.len();
    let lone_farmer = trader_stakes.first().copied().unwrap_or(0);
    println!("================ 9 NORMAL TRADERS vs 1 FARMER ================");
    println!("independent trader participants          : {n}  (each a distinct anon owner)");
    println!("a LONE farmer's individual capture       : {lone_farmer}  = {:.1}% of the trader cohort ({trader_supply})", lone_farmer as f64 * 100.0 / trader_supply as f64);
    println!("  -> 9 normal traders' REAL losses dilute the 1 farmer to ~1/{n} of the cohort. The farmer's only");
    println!("     way to beat the dilution is to Sybil into more accounts — but no per-participant cap can");
    println!("     stop that under anonymity; each Sybil account pays its OWN locked-capital + per-trade fee, so");
    println!("     the cost scales with the farm. That Sybil-flat cost (not a cap) is the bound.");
    println!("normal trader vs farmer, same {:.0}% COIN  : the 9 NORMAL traders LOST real capital for it (directional", lone_farmer as f64 * 100.0 / trader_supply as f64);
    println!("     risk); the farmer's net capital is ~0 (delta-neutral) but it is taxed by the fee + must lock");
    println!("     margin for the time-weighted tenure to earn at all.");
    println!("=============================================================\n");
    // A lone farmer is diluted to at most 1/N of the cohort (the per-stake share), NOT the Sybil aggregate.
    assert!(lone_farmer as u128 <= trader_supply / n as u128 + 1, "a lone farmer among {n} is diluted to <= 1/{n} of the cohort");
    let _ = lp_trader_supply;

    // Σ spent is 0: the delta-neutral wash is a REAL backstop drain, not a counterparty recovery, so the
    // spent-netting (trader = crystallized - spent) does NOT zero it — confirming the fee is the needed bound.
    assert_eq!(market_spent, 0, "delta-neutral wash leaves spent=0; spent-netting cannot catch it");
    // The fee skims FEE_BPS of the capture: the miner now takes (1 - fee) of the trader cohort, not ~all of it.
    let expected = trader_supply * (10_000 - FEE_BPS) as u128 / 10_000;
    assert!(miner_coin as u128 >= expected * 98 / 100 && (miner_coin as u128) <= expected,
        "post-fee capture should be ~{expected} ((1-{}%) of {trader_supply}); got {miner_coin}", FEE_BPS / 100);
}

// CHURN vs HOLD (validates the time-weight's spent-netting churn-penalty against the REAL percolator). Two
// identical delta-neutral miners crystallize the same loss on their long leg. The HOLDER keeps the loss leg
// OPEN -> nobody spends its budget -> spent stays 0 -> net = crystallized (full reward). The CHURNER recycles
// capital (closes then REOPENS the long) -> its OWN reopen fill posts new margin that SPENDS its crystallized
// budget -> spent rises -> net = crystallized - spent < the holder's. So "close and open -> you spend your own
// budget and net less" — exactly the disincentive the time-weight + net-by-spent design relies on.
#[test]
fn churn_raises_own_spent_and_collapses_the_net_reward_vs_a_holder() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    let payer = Keypair::new(); svm.airdrop(&payer.pubkey(), 100_000_000_000_000).unwrap();
    let oracle = Keypair::new(); svm.airdrop(&oracle.pubkey(), 1_000_000_000_000).unwrap();
    svm.set_sysvar(&Clock { slot: 100, unix_timestamp: 100, ..Default::default() });
    let collateral = create_real_mint(&mut svm, &payer, &Keypair::new().pubkey());
    let initial_price = 1_000u64;
    let mlen = percolator_prog::state::market_account_len_for_capacity(1).unwrap();
    let plen = percolator_prog::state::portfolio_account_len_for_market_slots(2).unwrap();
    let posq = (percolator::POS_SCALE / 2) as i128;
    let pk = payer.insecure_clone();
    let tx = move |svm: &mut LiteSVM, ixs: &[Instruction], extra: &[&Keypair]| {
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        let mut s: Vec<&Keypair> = vec![&pk]; s.extend_from_slice(extra);
        svm.send_transaction(Transaction::new_signed_with_payer(ixs, Some(&pk.pubkey()), &s, bh))
    };

    // Build a market + a delta-neutral pair (long+short). Returns (market, long_pf, short_pf, long, short).
    let imkt = || PIx::InitMarket { max_portfolio_assets: 1, h_min: 0, h_max: 10, initial_price,
        min_nonzero_mm_req: 1, min_nonzero_im_req: 2, maintenance_margin_bps: 10_000, initial_margin_bps: 10_000,
        max_trading_fee_bps: 10_000, trade_fee_base_bps: 3, liquidation_fee_bps: 0, liquidation_fee_cap: 0,
        min_liquidation_abs: 0, max_price_move_bps_per_slot: 10_000, max_accrual_dt_slots: 1,
        max_abs_funding_e9_per_slot: 0, min_funding_lifetime_slots: 1, max_account_b_settlement_chunks: 1,
        max_bankrupt_close_chunks: 1, max_bankrupt_close_lifetime_slots: 100, public_b_chunk_atoms: percolator::MAX_VAULT_TVL,
        maintenance_fee_per_slot: 0 };
    let setup_pair = |svm: &mut LiteSVM| -> (Pubkey, Pubkey, Pubkey, Keypair, Keypair) {
        let market = Pubkey::new_unique();
        svm.set_account(market, Account { lamports: 1_000_000_000, data: vec![0u8; mlen], owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
        let pv = canonical_insurance_vault(&perc_vault_authority(&market), &collateral);
        set_token(svm, &pv, &collateral, &perc_vault_authority(&market), 0);
        tx(svm, &[pix(vec![AccountMeta::new(oracle.pubkey(), true), AccountMeta::new(market, false), AccountMeta::new_readonly(collateral, false)], imkt())], &[&oracle]).expect("init market");
        tx(svm, &[pix(vec![AccountMeta::new(oracle.pubkey(), true), AccountMeta::new(market, false)], PIx::ConfigureAuthMark { asset_index: 0, now_slot: 100, initial_mark_e6: initial_price })], &[&oracle]).expect("cfg mark");
        let long = Keypair::new(); let short = Keypair::new();
        svm.airdrop(&long.pubkey(), 1_000_000_000).unwrap(); svm.airdrop(&short.pubkey(), 1_000_000_000).unwrap();
        let long_pf = Pubkey::new_unique(); let short_pf = Pubkey::new_unique();
        for (o, pf) in [(&short, &short_pf), (&long, &long_pf)] {
            svm.set_account(*pf, Account { lamports: 1_000_000_000, data: vec![0u8; plen], owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
            tx(svm, &[pix(vec![AccountMeta::new(o.pubkey(), true), AccountMeta::new(market, false), AccountMeta::new(*pf, false)], PIx::InitPortfolio)], &[o]).expect("init pf");
            let src = Pubkey::new_unique(); set_token(svm, &src, &collateral, &o.pubkey(), 1_000_000);
            tx(svm, &[pix(vec![AccountMeta::new(o.pubkey(), true), AccountMeta::new(market, false), AccountMeta::new(*pf, false), AccountMeta::new(src, false), AccountMeta::new(pv, false), AccountMeta::new_readonly(spl_token::ID, false)], PIx::Deposit { amount: 1_000_000 })], &[o]).expect("deposit");
        }
        tx(svm, &[pix(vec![AccountMeta::new(short.pubkey(), true), AccountMeta::new(long.pubkey(), true), AccountMeta::new(market, false), AccountMeta::new(short_pf, false), AccountMeta::new(long_pf, false)], PIx::TradeNoCpi { asset_index: 0, size_q: -posq, exec_price: initial_price, fee_bps: 0 })], &[&short, &long]).expect("open pair");
        (market, long_pf, short_pf, long, short)
    };
    let crank = |svm: &mut LiteSVM, market: &Pubkey, pf: &Pubkey, action: u8| {
        tx(svm, &[pix(vec![AccountMeta::new(payer.pubkey(), true), AccountMeta::new(*market, false), AccountMeta::new(*pf, false)],
            PIx::PermissionlessCrank { action, asset_index: 0, now_slot: 110, funding_rate_e9: 0, close_q: 0, fee_bps: 0, recovery_reason: 0 })], &[]).expect("crank");
    };

    let (h_market, h_long, h_short, _hl, _hs) = setup_pair(&mut svm);
    let (c_market, c_long, c_short, c_lk, c_sk) = setup_pair(&mut svm);

    // The neutral oracle drops both marks; the crank crystallizes both longs' losses identically.
    svm.set_sysvar(&Clock { slot: 110, unix_timestamp: 110, ..Default::default() });
    for (m, lpf, spf) in [(h_market, h_long, h_short), (c_market, c_long, c_short)] {
        tx(&mut svm, &[pix(vec![AccountMeta::new(oracle.pubkey(), true), AccountMeta::new(m, false)], PIx::PushAuthMark { asset_index: 0, now_slot: 110, mark_e6: initial_price / 2 })], &[&oracle]).expect("drop mark");
        for pf in [spf, lpf] { crank(&mut svm, &m, &pf, 2); crank(&mut svm, &m, &pf, 0); }
    }
    let h_cryst = read_crystallized(&svm, &h_long);
    let c_cryst = read_crystallized(&svm, &c_long);
    assert!(h_cryst > 0 && c_cryst == h_cryst, "both longs crystallized the SAME loss: h={h_cryst} c={c_cryst}");
    assert_eq!(read_spent(&svm, &h_long), 0, "holder's budget is untouched (spent 0)");
    assert_eq!(read_spent(&svm, &c_long), 0, "churner's budget is untouched BEFORE churning");

    // CHURN: the churner recycles capital — close the pair (reverse), then REOPEN it. The reopen posts new
    // margin on the long, which SPENDS its own crystallized budget -> spent rises.
    tx(&mut svm, &[pix(vec![AccountMeta::new(c_sk.pubkey(), true), AccountMeta::new(c_lk.pubkey(), true), AccountMeta::new(c_market, false), AccountMeta::new(c_short, false), AccountMeta::new(c_long, false)], PIx::TradeNoCpi { asset_index: 0, size_q: posq, exec_price: initial_price / 2, fee_bps: 0 })], &[&c_sk, &c_lk]).expect("close");
    tx(&mut svm, &[pix(vec![AccountMeta::new(c_sk.pubkey(), true), AccountMeta::new(c_lk.pubkey(), true), AccountMeta::new(c_market, false), AccountMeta::new(c_short, false), AccountMeta::new(c_long, false)], PIx::TradeNoCpi { asset_index: 0, size_q: -posq, exec_price: initial_price / 2, fee_bps: 0 })], &[&c_sk, &c_lk]).expect("reopen");
    crank(&mut svm, &c_market, &c_long, 2); crank(&mut svm, &c_market, &c_long, 0);

    let h_spent = read_spent(&svm, &h_long);
    let c_spent = read_spent(&svm, &c_long);
    let h_net = h_cryst.saturating_sub(h_spent);
    let c_net = read_crystallized(&svm, &c_long).saturating_sub(c_spent);
    println!("\n================ CHURN vs HOLD ================");
    println!("HOLDER  long: crystallized {h_cryst}, spent {h_spent}  -> NET {h_net}  (kept the leg open)");
    println!("CHURNER long: crystallized {} , spent {c_spent}  -> NET {c_net}  (closed + reopened = recycled capital)", read_crystallized(&svm, &c_long));
    println!("=> churning spent its OWN budget; net-by-spent reward {} the holder's", if c_net < h_net { "BELOW" } else { "NOT below" });
    println!("===============================================\n");
    assert!(c_spent > 0, "churn (close+reopen) raises the churner's OWN spent");
    assert!(c_net < h_net, "the churner's net-by-spent reward ({c_net}) is below the holder's ({h_net})");
}

// SURFACE A — buy/burn FUEL verification (sweep tick): the genesis market is configured with trade_fee_base_bps=3
// + fee_redirect_to_market_0_bps=2000 ("3 bps/trade, 20% of yield -> asset-0 insurance"). Two things were only
// asserted at INIT (read-back), never on a real trade:
//   (1) percolator charges the market base fee even when the caller passes fee_bps=0 — hybrid_trade_fee_bps_view
//       takes max(caller_fee_bps, cfg.trade_fee_base_bps) (percolator v16_program.rs:11224). So EVERY trade pays
//       >= 3 bps, and that fee lands in asset-0 insurance — the real, recurring input to surplus -> pull -> buy/burn.
//   (2) the 20% redirect is INERT on a single-asset genesis market: credit_fee_to_domain_budget_view forces
//       redirect=0 when asset_index==0 (v16_program.rs:5252) — there is no OTHER asset to redirect FROM, so
//       asset-0 simply keeps 100% of its own fees. The redirect only matters once the market has assets 1..n.
// This pins the load-bearing claim that fees actually accrue to asset-0 insurance on real trades (without it the
// whole buy/burn loop would have no fuel), and corrects the sim's stale "trading fees 0 here" note: they are NOT 0.
#[test]
fn genesis_market_3bps_fee_accrues_to_asset0_insurance_on_a_real_trade_redirect_inert_for_asset0() {
    let mut svm = LiteSVM::new().with_compute_budget(solana_program_runtime::compute_budget::ComputeBudget {
        compute_unit_limit: 1_400_000, heap_size: 256 * 1024,
        ..solana_program_runtime::compute_budget::ComputeBudget::default()
    });
    svm.add_program_from_file(perc_id(), perc_so()).unwrap();
    let payer = Keypair::new(); svm.airdrop(&payer.pubkey(), 100_000_000_000_000).unwrap();
    let oracle = Keypair::new(); svm.airdrop(&oracle.pubkey(), 1_000_000_000_000).unwrap();
    svm.set_sysvar(&Clock { slot: 100, unix_timestamp: 100, ..Default::default() });
    let collateral = create_real_mint(&mut svm, &payer, &Keypair::new().pubkey());
    // High price so the proven small position size carries enough notional for a non-zero 3 bps fee:
    // notional = (POS_SCALE/2)*100_000/POS_SCALE = 50_000 per side -> fee ~= 50_000*3/10_000 = 15 per side.
    let initial_price = 100_000u64;
    let mlen = percolator_prog::state::market_account_len_for_capacity(1).unwrap();
    let plen = percolator_prog::state::portfolio_account_len_for_market_slots(2).unwrap();
    let posq = (percolator::POS_SCALE / 2) as i128;
    let pk = payer.insecure_clone();
    let tx = move |svm: &mut LiteSVM, ixs: &[Instruction], extra: &[&Keypair]| {
        svm.expire_blockhash(); let bh = svm.latest_blockhash();
        let mut s: Vec<&Keypair> = vec![&pk]; s.extend_from_slice(extra);
        svm.send_transaction(Transaction::new_signed_with_payer(ixs, Some(&pk.pubkey()), &s, bh))
    };
    let imkt = || PIx::InitMarket { max_portfolio_assets: 1, h_min: 0, h_max: 10, initial_price,
        min_nonzero_mm_req: 1, min_nonzero_im_req: 2, maintenance_margin_bps: 10_000, initial_margin_bps: 10_000,
        max_trading_fee_bps: 10_000, trade_fee_base_bps: 3, liquidation_fee_bps: 0, liquidation_fee_cap: 0,
        min_liquidation_abs: 0, max_price_move_bps_per_slot: 10_000, max_accrual_dt_slots: 1,
        max_abs_funding_e9_per_slot: 0, min_funding_lifetime_slots: 1, max_account_b_settlement_chunks: 1,
        max_bankrupt_close_chunks: 1, max_bankrupt_close_lifetime_slots: 100, public_b_chunk_atoms: percolator::MAX_VAULT_TVL,
        maintenance_fee_per_slot: 0 };
    let market = Pubkey::new_unique();
    svm.set_account(market, Account { lamports: 1_000_000_000, data: vec![0u8; mlen], owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
    let pv = canonical_insurance_vault(&perc_vault_authority(&market), &collateral);
    set_token(&mut svm, &pv, &collateral, &perc_vault_authority(&market), 0);
    tx(&mut svm, &[pix(vec![AccountMeta::new(oracle.pubkey(), true), AccountMeta::new(market, false), AccountMeta::new_readonly(collateral, false)], imkt())], &[&oracle]).expect("init market");
    tx(&mut svm, &[pix(vec![AccountMeta::new(oracle.pubkey(), true), AccountMeta::new(market, false)], PIx::ConfigureAuthMark { asset_index: 0, now_slot: 100, initial_mark_e6: initial_price })], &[&oracle]).expect("cfg mark");

    let long = Keypair::new(); let short = Keypair::new();
    svm.airdrop(&long.pubkey(), 1_000_000_000).unwrap(); svm.airdrop(&short.pubkey(), 1_000_000_000).unwrap();
    let long_pf = Pubkey::new_unique(); let short_pf = Pubkey::new_unique();
    for (o, pf) in [(&short, &short_pf), (&long, &long_pf)] {
        svm.set_account(*pf, Account { lamports: 1_000_000_000, data: vec![0u8; plen], owner: perc_id(), executable: false, rent_epoch: 0 }).unwrap();
        tx(&mut svm, &[pix(vec![AccountMeta::new(o.pubkey(), true), AccountMeta::new(market, false), AccountMeta::new(*pf, false)], PIx::InitPortfolio)], &[o]).expect("init pf");
        let src = Pubkey::new_unique(); set_token(&mut svm, &src, &collateral, &o.pubkey(), 1_000_000);
        tx(&mut svm, &[pix(vec![AccountMeta::new(o.pubkey(), true), AccountMeta::new(market, false), AccountMeta::new(*pf, false), AccountMeta::new(src, false), AccountMeta::new(pv, false), AccountMeta::new_readonly(spl_token::ID, false)], PIx::Deposit { amount: 1_000_000 })], &[o]).expect("deposit");
    }

    let ins_before = read_insurance(&svm, &market);
    // One real delta-neutral trade, caller passes fee_bps=0 — the market's 3 bps base still applies.
    tx(&mut svm, &[pix(vec![AccountMeta::new(short.pubkey(), true), AccountMeta::new(long.pubkey(), true), AccountMeta::new(market, false), AccountMeta::new(short_pf, false), AccountMeta::new(long_pf, false)], PIx::TradeNoCpi { asset_index: 0, size_q: -posq, exec_price: initial_price, fee_bps: 0 })], &[&short, &long]).expect("open pair");
    let ins_after = read_insurance(&svm, &market);

    println!("\n=== GENESIS MARKET FEE -> ASSET-0 INSURANCE (buy/burn fuel) ===");
    println!("asset-0 insurance before trade : {ins_before}");
    println!("asset-0 insurance after  trade : {ins_after}  (+{} from the 3 bps base fee, caller passed fee_bps=0)", ins_after - ins_before);
    println!("redirect (fee_redirect_to_market_0_bps) is INERT for asset_index==0 — asset-0 keeps 100% of its own fee");
    println!("==============================================================\n");
    assert!(ins_after > ins_before, "a real trade must accrue the 3 bps base fee to asset-0 insurance — the buy/burn fuel (got {ins_before} -> {ins_after})");
}
