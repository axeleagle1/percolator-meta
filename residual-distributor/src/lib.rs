//! Deterministic, points-based COIN distribution decider.
//!
//! **Branch `risidual_genesis_never_push_upstream` — do NOT push upstream.**
//!
//! Drop-in alternative to `genesis-vote` behind the `distribution` program's
//! pluggable-decider seam (see distribution/src/lib.rs "Decider seam"). The winning
//! COIN allocation is computed **deterministically** from residual-backing points,
//! so there is nothing for a late whale to capture: every backer's share is fixed
//! by the risk it actually bore.
//!
//! ## Points source — percolator counters via snapshot-delta (zero ledgers in percolator)
//!
//! Per `/tmp/prog.md` (capped-counter-transfer model), percolator keeps monotonic
//! per-backer scalars and NO ledger: `residual_received` = `cumulative_loss_atoms`,
//! fee-support = `total_earnings_atoms`, backing = `total_principal_atoms`. A backer
//! registers a START snapshot here; CRYSTALLIZE reads the END snapshot and credits
//! `eligible = min(Δresidual, Δfee*10000/bps)` weighted by `floor(log2(end-start))`.
//! Conservation is enforced by percolator at the sink; the fee cap defeats wash;
//! the hold-window is computed here, so JIT capture is damped with no percolator
//! start-slot field.
//!
//! ## Decision = verify-then-seal
//! A cranker creates+appends the distribution proposal with the deterministic
//! entries (funded by the cranker). `IX_SEAL` **re-derives** each entry from the
//! on-chain PointStake accounts and refuses to seal unless every `(recipient,
//! amount)` matches `amount = floor(total_supply * points_i / total_points)`. Then
//! it CPIs `distribution::seal_winner` signed by this program's config PDA (the
//! distribution authority). Determinism is enforced on-chain; nothing is trusted.

#![no_std]
extern crate alloc;
#[allow(unused_imports)]
use alloc::format; // required by entrypoint!/msg! in SBF builds

use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
    declare_id,
    entrypoint::ProgramResult,
    program::invoke_signed,
    program_error::ProgramError,
    program_pack::Pack,
    pubkey::Pubkey,
    rent::Rent,
    system_instruction,
    sysvar::Sysvar,
};

declare_id!("Res1dua1Distr1butor111111111111111111111111");

const BPS_DENOMINATOR: u64 = 10_000;
pub const DEFAULT_FEE_SUPPORT_BPS: u16 = 80;

// The ONE deployed distribution program this decider CPIs (finding HK). Pinning it closes the
// HC-residual init-squat flavor: HC binds distribution_config to the canonical PDA *under the passed
// distribution_program*, but a front-runner could pass a FAKE program (deriving the canonical config
// under it) so seal would CPI the fake program and the real COIN-holding distribution would never be
// sealed -> DOS. Synced to distribution_program::id() by tests/offsets.rs.
pub const DISTRIBUTION_PROGRAM_ID: Pubkey =
    solana_program::pubkey!("D1str1but1on11111111111111111111111111111111");

const CONFIG_DISC: [u8; 8] = *b"RDCONFG1";
const STAKE_DISC: [u8; 8] = *b"RDSTAKE1";
const CONFIG_SIZE: usize = 466; // base 366 + 4-cohort tail (backing_pool 32 + 4 bps/points fields = 100)
const STAKE_SIZE: usize = 211; // +1 claimed flag (self-service)
// 4-cohort deterministic model (10/10/40/40). Insurance & backing reward SHARE VALUE (subledger
// Position.shares — pro-rata with fees, soft-veto on exit); LP & trader reward the percolator
// PortfolioAccountV16 residual counters (received / crystallized_loss — monotonic, real-loss-backed,
// un-gameable). See tests/offsets.rs for the pinned reads.
const COHORT_INSURANCE: u8 = 0;
const COHORT_BACKING: u8 = 1;
const COHORT_LP: u8 = 2;
const COHORT_TRADER: u8 = 3;

const IX_INIT: u8 = 0;
const IX_REGISTER_START: u8 = 1;
const IX_CRYSTALLIZE: u8 = 2;
// tag 3 (legacy cranker IX_SEAL) RETIRED — superseded by the self-service freeze+claim path below.
// Self-service path (replacing the cranker seal). After emission_end, IX_FREEZE snapshots the
// cohort denominators and closes register/crystallize; backers then finalize/claim their own share.
const IX_FREEZE: u8 = 4;
const IX_CLAIM: u8 = 5;

// ===========================================================================
// Deterministic, gaming-resistant point math  (pure — unit-tested below)
// ===========================================================================

/// Deterministic pro-rata split; floor rounding never over-allocates the fixed pool.
pub fn points_to_amount(total_supply: u64, points_i: u128, total_points: u128) -> u64 {
    if total_points == 0 {
        return 0;
    }
    ((total_supply as u128).saturating_mul(points_i) / total_points) as u64
}

// percolator account header length (KIND/version/etc.) — all percolator account reads below are at
// PERC_HEADER_LEN + within-struct offset, PINNED against the real structs by tests/offsets.rs
// (offset_of! + HEADER_LEN), finding-T discipline.
pub const PERC_HEADER_LEN: usize = 16;

fn read_u128(data: &[u8], off: usize) -> Result<u128, ProgramError> {
    let b = data.get(off..off + 16).ok_or(ProgramError::AccountDataTooSmall)?;
    Ok(u128::from_le_bytes(b.try_into().unwrap()))
}

// ===========================================================================
// Insurance cohort (the SOFT VETO half) — points read LIVE from the subledger
// position, so an exit (principal -> 0, withdrawn) AUTO-FORFEITS the COIN share.
// ===========================================================================
// subledger Position offsets (stable across the share-model change — appended
// fields only): principal u64@72, withdrawn u8@88, start_slot u64@89.
// Subledger Position offsets. PINNED against the subledger's exported POS_* consts by
// tests/offsets.rs (finding HF: a wrong owner offset here slipped past mocked tests).
pub const SUB_POS_POOL: usize = 8; // Position.pool @ 8 (real layout: disc@0, pool@8..40, owner@40..72).
pub const SUB_POS_OWNER: usize = 40; // Position.owner @ 40. The depositor owed this position's COIN.
pub const SUB_POS_WITHDRAWN: usize = 88;
// Position.shares (POLICY_WITH_SURPLUS) @104 — the SHARE-VALUE points source for the insurance AND
// backing cohorts. Within one pool the share price (balance/total_shares) is common, so pro-rata by
// share value == pro-rata by shares; shares also encode the fee/time weighting (an earlier depositor
// holds more shares per dollar) and give the soft-veto for free (exit redeems shares -> 0 -> forfeit).
pub const SUB_POS_SHARES: usize = 104;

/// (shares, withdrawn) from a live subledger Position — the SHARE-VALUE points for the insurance &
/// backing cohorts. A withdrawn (or zero-share) position yields 0 (soft veto): an exiter redeemed its
/// shares, forfeiting its COIN. Read LIVE at claim so a partial redeem can't over-claim.
pub fn read_subledger_shares(data: &[u8]) -> Result<(u128, bool), ProgramError> {
    let shares = read_u128(data, SUB_POS_SHARES)?;
    let withdrawn = *data.get(SUB_POS_WITHDRAWN).ok_or(ProgramError::AccountDataTooSmall)? == 1;
    Ok((shares, withdrawn))
}

/// Share-value points: just the live shares (0 if exited). Pro-rata across the cohort's pool.
pub fn share_value_points(shares: u128, withdrawn: bool) -> u128 {
    if withdrawn {
        0
    } else {
        shares
    }
}

// ===========================================================================
// percolator PortfolioAccountV16Account snapshot read (LP & trader cohorts) — offsets PINNED
// ===========================================================================
// Account = HEADER_LEN(16) + repr(C) PortfolioAccountV16Account { provenance_header(100), owner[32]@100,
// capital@132, pnl@148, reserved_pnl@164, residual_crystallized_loss_atoms_total@180,
// residual_spent_principal_atoms_total@196, residual_received_atoms_total@212, ... }. Absolute = 16 +
// within-struct. PINNED against the real struct by tests/offsets.rs (offset_of! + HEADER_LEN).
// LP cohort reads `received` (residual the matcher absorbed); trader cohort reads `crystallized_loss`
// (real loss the account took). Both are monotonic + backed by REAL crystallized loss (spent<=crystallized,
// shape-validated), so they cannot be farmed without actually losing money (un-gameable, vs fees).
// The portfolio's provenance market_group_id is the FIRST field of the struct, so it sits right after
// the percolator account header. The LP/trader cohorts MUST scope to it (finding IL): the residual
// counters are admin-mark-manipulable on a market whose oracle the registrant controls, so a portfolio
// is only countable if it belongs to the ONE allow-listed (trusted-Pyth) genesis market the orchestrator
// bound at init (config.market_group). Without this an attacker stands up their OWN percolator market with
// an auth-mark oracle they push, self-trades to mint crystallized_loss/received, and farms the COIN.
pub const OFF_PORTFOLIO_MARKET_GROUP: usize = PERC_HEADER_LEN;
pub const OFF_PORTFOLIO_OWNER: usize = PERC_HEADER_LEN + 100;
pub const OFF_PORTFOLIO_CRYSTALLIZED_LOSS: usize = PERC_HEADER_LEN + 180;
pub const OFF_PORTFOLIO_RECEIVED: usize = PERC_HEADER_LEN + 212;

/// (residual_received, residual_crystallized_loss) from a live percolator PortfolioAccount.
pub fn read_portfolio_residual(data: &[u8]) -> Result<(u128, u128), ProgramError> {
    Ok((
        read_u128(data, OFF_PORTFOLIO_RECEIVED)?,
        read_u128(data, OFF_PORTFOLIO_CRYSTALLIZED_LOSS)?,
    ))
}

// ===========================================================================
// State
// ===========================================================================
struct Config {
    coin_mint: Pubkey,
    distribution_program: Pubkey,
    distribution_config: Pubkey,
    percolator_program: Pubkey,
    total_supply: u64,
    fee_support_bps: u16,
    emission_end_slot: u64,
    total_points: u128,        // residual-backing cohort
    sealed: u8,
    bump: u8,
    insurance_bps: u16,        // insurance cohort's share of supply (e.g. 2000 = 20%)
    insurance_total_points: u128, // insurance cohort total (capital*log-time)
    subledger_program: Pubkey, // owner of the insurance-cohort positions
    // The ONE genesis insurance pool the insurance cohort is scoped to (finding HG). An insurance
    // position from any OTHER pool of the same subledger program must not farm this genesis's COIN.
    subledger_pool: Pubkey,
    // The genesis percolator market_group the RESIDUAL cohort is scoped to (finding HI). A backing
    // ledger from any OTHER market must not farm this genesis's COIN. Pubkey::default() = unscoped.
    market_group: Pubkey,
    // SELF-SERVICE FINALIZE (replacing the cranker seal). After emission_end a permissionless
    // IX_FREEZE snapshots the cohort denominators here and stamps freeze_slot; from then on
    // register/crystallize are closed and each backer finalizes/claims their OWN deterministic share
    // (share = cohort_supply * points / frozen_*_points). freeze_slot == 0 means "not yet frozen".
    frozen_total_points: u128,
    frozen_insurance_total_points: u128,
    freeze_slot: u64,
    // The COIN vault (token account owned by this rd_config PDA) that self-service claims pay from.
    // Bound at freeze after verifying it is rd_config-owned, holds the full fixed supply, and the
    // coin_mint has no mint authority (GX/EZ) — so the supply can't be inflated under the claimers.
    // Pubkey::default() until frozen.
    vault: Pubkey,
    // Slots AFTER emission_end during which backers do their final crystallize before the denominators
    // lock. freeze is rejected until `emission_end + finalize_window`. Since freeze is PERMISSIONLESS,
    // a zero window would let anyone freeze the instant emission ends and forfeit slower backers' still
    // un-crystallized points; the orchestrator sets ~1 week here (the "finalize your points" window).
    finalize_window: u64,
    // ---- 4-cohort tail (10/10/40/40). `total_points`/`insurance_total_points` above are the BACKING
    // and INSURANCE cohort point totals; these add the LP and TRADER cohorts + their bps + the backing
    // pool scope. trader_bps is implicit (10000 - insurance - backing - lp). ----
    backing_pool: Pubkey,           // the genesis BACKING subledger pool (DOMAIN_BACKING) the backing cohort is scoped to
    backing_bps: u16,               // backing cohort supply share
    lp_bps: u16,                    // LP cohort supply share
    lp_total_points: u128,          // LP cohort (PortfolioAccount residual_received Δ)
    trader_total_points: u128,      // trader cohort (PortfolioAccount residual_crystallized_loss Δ)
    frozen_lp_total_points: u128,
    frozen_trader_total_points: u128,
}
impl Config {
    fn deserialize(d: &[u8]) -> Result<Self, ProgramError> {
        if d.len() < CONFIG_SIZE || d[..8] != CONFIG_DISC {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(Config {
            coin_mint: pk(d, 8),
            distribution_program: pk(d, 40),
            distribution_config: pk(d, 72),
            percolator_program: pk(d, 104),
            total_supply: u64::from_le_bytes(d[136..144].try_into().unwrap()),
            fee_support_bps: u16::from_le_bytes(d[144..146].try_into().unwrap()),
            emission_end_slot: u64::from_le_bytes(d[146..154].try_into().unwrap()),
            total_points: u128::from_le_bytes(d[154..170].try_into().unwrap()),
            sealed: d[170],
            bump: d[171],
            insurance_bps: u16::from_le_bytes(d[172..174].try_into().unwrap()),
            insurance_total_points: u128::from_le_bytes(d[174..190].try_into().unwrap()),
            subledger_program: pk(d, 190),
            subledger_pool: pk(d, 222),
            market_group: pk(d, 254),
            frozen_total_points: u128::from_le_bytes(d[286..302].try_into().unwrap()),
            frozen_insurance_total_points: u128::from_le_bytes(d[302..318].try_into().unwrap()),
            freeze_slot: u64::from_le_bytes(d[318..326].try_into().unwrap()),
            vault: pk(d, 326),
            finalize_window: u64::from_le_bytes(d[358..366].try_into().unwrap()),
            backing_pool: pk(d, 366),
            backing_bps: u16::from_le_bytes(d[398..400].try_into().unwrap()),
            lp_bps: u16::from_le_bytes(d[400..402].try_into().unwrap()),
            lp_total_points: u128::from_le_bytes(d[402..418].try_into().unwrap()),
            trader_total_points: u128::from_le_bytes(d[418..434].try_into().unwrap()),
            frozen_lp_total_points: u128::from_le_bytes(d[434..450].try_into().unwrap()),
            frozen_trader_total_points: u128::from_le_bytes(d[450..466].try_into().unwrap()),
        })
    }
    fn serialize(&self, d: &mut [u8]) {
        d[..8].copy_from_slice(&CONFIG_DISC);
        d[8..40].copy_from_slice(self.coin_mint.as_ref());
        d[40..72].copy_from_slice(self.distribution_program.as_ref());
        d[72..104].copy_from_slice(self.distribution_config.as_ref());
        d[104..136].copy_from_slice(self.percolator_program.as_ref());
        d[136..144].copy_from_slice(&self.total_supply.to_le_bytes());
        d[144..146].copy_from_slice(&self.fee_support_bps.to_le_bytes());
        d[146..154].copy_from_slice(&self.emission_end_slot.to_le_bytes());
        d[154..170].copy_from_slice(&self.total_points.to_le_bytes());
        d[170] = self.sealed;
        d[171] = self.bump;
        d[172..174].copy_from_slice(&self.insurance_bps.to_le_bytes());
        d[174..190].copy_from_slice(&self.insurance_total_points.to_le_bytes());
        d[190..222].copy_from_slice(self.subledger_program.as_ref());
        d[222..254].copy_from_slice(self.subledger_pool.as_ref());
        d[254..286].copy_from_slice(self.market_group.as_ref());
        d[286..302].copy_from_slice(&self.frozen_total_points.to_le_bytes());
        d[302..318].copy_from_slice(&self.frozen_insurance_total_points.to_le_bytes());
        d[318..326].copy_from_slice(&self.freeze_slot.to_le_bytes());
        d[326..358].copy_from_slice(self.vault.as_ref());
        d[358..366].copy_from_slice(&self.finalize_window.to_le_bytes());
        d[366..398].copy_from_slice(self.backing_pool.as_ref());
        d[398..400].copy_from_slice(&self.backing_bps.to_le_bytes());
        d[400..402].copy_from_slice(&self.lp_bps.to_le_bytes());
        d[402..418].copy_from_slice(&self.lp_total_points.to_le_bytes());
        d[418..434].copy_from_slice(&self.trader_total_points.to_le_bytes());
        d[434..450].copy_from_slice(&self.frozen_lp_total_points.to_le_bytes());
        d[450..466].copy_from_slice(&self.frozen_trader_total_points.to_le_bytes());
    }
    /// trader bps = the remainder (so the four cohorts always sum to exactly 100%).
    fn trader_bps(&self) -> u16 {
        (BPS_DENOMINATOR as u16)
            .saturating_sub(self.insurance_bps)
            .saturating_sub(self.backing_bps)
            .saturating_sub(self.lp_bps)
    }
    /// COIN supply allocated to a cohort.
    fn cohort_supply(&self, cohort: u8) -> u64 {
        let bps = match cohort {
            COHORT_INSURANCE => self.insurance_bps,
            COHORT_BACKING => self.backing_bps,
            COHORT_LP => self.lp_bps,
            _ => self.trader_bps(),
        } as u128;
        ((self.total_supply as u128) * bps / BPS_DENOMINATOR as u128) as u64
    }
    /// Live running point total for a cohort (mutated in register/crystallize).
    fn cohort_points_mut(&mut self, cohort: u8) -> &mut u128 {
        match cohort {
            COHORT_INSURANCE => &mut self.insurance_total_points,
            COHORT_BACKING => &mut self.total_points,
            COHORT_LP => &mut self.lp_total_points,
            _ => &mut self.trader_total_points,
        }
    }
    /// Frozen denominator for a cohort (snapshotted at freeze; used by claim).
    fn frozen_cohort_points(&self, cohort: u8) -> u128 {
        match cohort {
            COHORT_INSURANCE => self.frozen_insurance_total_points,
            COHORT_BACKING => self.frozen_total_points,
            COHORT_LP => self.frozen_lp_total_points,
            _ => self.frozen_trader_total_points,
        }
    }
}

struct Stake {
    config: Pubkey,
    owner: Pubkey,
    backing_ledger: Pubkey,
    recipient: Pubkey,
    residual_snap: u128,
    earnings_snap: u128,
    start_slot: u64,
    points: u128,
    bump: u8,
    cohort: u8, // COHORT_RESIDUAL | COHORT_INSURANCE. For insurance, `backing_ledger` is the
                // subledger position and `recipient` is the depositor.
    // Running sum of fee-supported eligible residual across crystallize windows. The tenure
    // multiplier is applied to THIS total against the original start_slot, so points are
    // independent of crystallize cadence (anti-grief, finding GZ).
    eligible_accum: u128,
    // Self-service claim: set true when this stake's COIN share has been paid, so it can't be
    // double-claimed.
    claimed: bool,
}
impl Stake {
    fn deserialize(d: &[u8]) -> Result<Self, ProgramError> {
        if d.len() < STAKE_SIZE || d[..8] != STAKE_DISC {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(Stake {
            config: pk(d, 8),
            owner: pk(d, 40),
            backing_ledger: pk(d, 72),
            recipient: pk(d, 104),
            residual_snap: u128::from_le_bytes(d[136..152].try_into().unwrap()),
            earnings_snap: u128::from_le_bytes(d[152..168].try_into().unwrap()),
            start_slot: u64::from_le_bytes(d[168..176].try_into().unwrap()),
            points: u128::from_le_bytes(d[176..192].try_into().unwrap()),
            bump: d[192],
            cohort: d[193],
            eligible_accum: u128::from_le_bytes(d[194..210].try_into().unwrap()),
            claimed: d[210] != 0,
        })
    }
    fn serialize(&self, d: &mut [u8]) {
        d[..8].copy_from_slice(&STAKE_DISC);
        d[8..40].copy_from_slice(self.config.as_ref());
        d[40..72].copy_from_slice(self.owner.as_ref());
        d[72..104].copy_from_slice(self.backing_ledger.as_ref());
        d[104..136].copy_from_slice(self.recipient.as_ref());
        d[136..152].copy_from_slice(&self.residual_snap.to_le_bytes());
        d[152..168].copy_from_slice(&self.earnings_snap.to_le_bytes());
        d[168..176].copy_from_slice(&self.start_slot.to_le_bytes());
        d[176..192].copy_from_slice(&self.points.to_le_bytes());
        d[192] = self.bump;
        d[193] = self.cohort;
        d[194..210].copy_from_slice(&self.eligible_accum.to_le_bytes());
        d[210] = self.claimed as u8;
    }
}

fn pk(d: &[u8], off: usize) -> Pubkey {
    Pubkey::new_from_array(d[off..off + 32].try_into().unwrap())
}

fn config_seeds<'a>(coin_mint: &'a Pubkey) -> [&'a [u8]; 2] {
    [b"rd_config", coin_mint.as_ref()]
}

// ===========================================================================
#[cfg(not(feature = "no-entrypoint"))]
solana_program::entrypoint!(process_instruction);

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let (tag, rest) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    match *tag {
        IX_INIT => init(program_id, accounts, rest),
        IX_REGISTER_START => register_start(program_id, accounts, rest),
        IX_CRYSTALLIZE => crystallize(program_id, accounts),
        IX_FREEZE => freeze(program_id, accounts),
        IX_CLAIM => claim(program_id, accounts),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn create_pda<'a>(
    payer: &AccountInfo<'a>,
    target: &AccountInfo<'a>,
    system: &AccountInfo<'a>,
    program_id: &Pubkey,
    seeds: &[&[u8]],
    size: usize,
) -> ProgramResult {
    let rent = Rent::get()?.minimum_balance(size);
    invoke_signed(
        &system_instruction::create_account(payer.key, target.key, rent, size as u64, program_id),
        &[payer.clone(), target.clone(), system.clone()],
        &[seeds],
    )
}

// init accounts: [payer(s,w), coin_mint, distribution_program, distribution_config,
//   percolator_program, subledger_program, config(pda,w), system]
// data: total_supply(u64), fee_support_bps(u16), emission_end_slot(u64), insurance_bps(u16)
fn init(program_id: &Pubkey, accounts: &[AccountInfo], mut data: &[u8]) -> ProgramResult {
    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let coin_mint = next_account_info(iter)?;
    let distribution_program = next_account_info(iter)?;
    let distribution_config = next_account_info(iter)?;
    let percolator_program = next_account_info(iter)?;
    let subledger_program = next_account_info(iter)?;
    let config_account = next_account_info(iter)?;
    let system = next_account_info(iter)?;

    // 4-cohort wire (10/10/40/40 default): total_supply, emission_end, insurance_bps, backing_bps,
    // lp_bps (trader = remainder), finalize_window, subledger_pool (insurance), backing_pool, market_group.
    let total_supply = take_u64(&mut data)?;
    let emission_end_slot = take_u64(&mut data)?;
    let insurance_bps = take_u16(&mut data)?;
    let backing_bps = take_u16(&mut data)?;
    let lp_bps = take_u16(&mut data)?;
    let finalize_window = take_u64(&mut data)?;
    let subledger_pool = take_pubkey(&mut data)?; // insurance pool (DOMAIN_INSURANCE), finding HG scope
    let backing_pool = take_pubkey(&mut data)?; // backing pool (DOMAIN_BACKING) scope
    let market_group = take_pubkey(&mut data)?; // genesis market (informational scope for LP/trader)
    if !data.is_empty() || !payer.is_signer || total_supply == 0 {
        return Err(ProgramError::InvalidInstructionData);
    }
    // The four cohort shares must not exceed 100% (trader takes the remainder, so sum <= 10000).
    if (insurance_bps as u32) + (backing_bps as u32) + (lp_bps as u32) > BPS_DENOMINATOR as u32 {
        return Err(ProgramError::InvalidInstructionData);
    }
    // A cohort with a share MUST be scoped to its concrete pool, else a position from any other pool of
    // the same subledger program could farm this genesis's COIN.
    if insurance_bps > 0 && subledger_pool == Pubkey::default() {
        return Err(ProgramError::InvalidInstructionData);
    }
    if backing_bps > 0 && backing_pool == Pubkey::default() {
        return Err(ProgramError::InvalidInstructionData);
    }
    // The LP and trader cohorts read percolator portfolio counters that are admin-mark-manipulable on a
    // market whose oracle the registrant controls, so they MUST be scoped to the one allow-listed
    // trusted-Pyth genesis market (finding IL). If either cohort has a share (lp_bps > 0, or trader =
    // the remainder > 0), market_group must be a real key — never default (which register treats as
    // "any market" and would let an attacker's self-oracle'd market farm the COIN).
    let trader_bps = (BPS_DENOMINATOR as u32)
        .saturating_sub(insurance_bps as u32 + backing_bps as u32 + lp_bps as u32);
    if (lp_bps > 0 || trader_bps > 0) && market_group == Pubkey::default() {
        return Err(ProgramError::InvalidInstructionData);
    }
    let (expected, bump) = Pubkey::find_program_address(&config_seeds(coin_mint.key), program_id);
    if *config_account.key != expected || config_account.data_len() != 0 {
        return Err(ProgramError::InvalidSeeds);
    }
    // Pin the distribution program (finding HK): a fake program would let a front-runner squat with a
    // canonical-looking-but-foreign distribution_config and brick the real COIN distribution at seal.
    if *distribution_program.key != DISTRIBUTION_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    // Bind distribution_config to the canonical PDA(["dist_config", coin_mint, rd_config]) under the
    // distribution program (finding HC; parity with genesis-vote finding R). rd_config (= `expected`)
    // is the distribution authority, so the ONLY config rd can ever seal is the one at this PDA.
    // Without this, a front-runner could squat this canonical (per-coin_mint) rd_config with a foreign
    // distribution_config; since rd_config can't be re-initialized, seal would forever target the
    // foreign config and the real COIN-holding distribution could never be sealed -> DOS.
    let (expected_dist, _) = Pubkey::find_program_address(
        &[b"dist_config", coin_mint.key.as_ref(), expected.as_ref()],
        distribution_program.key,
    );
    if *distribution_config.key != expected_dist {
        return Err(ProgramError::InvalidSeeds);
    }
    let bump_arr = [bump];
    let seeds: [&[u8]; 3] = [b"rd_config", coin_mint.key.as_ref(), &bump_arr];
    create_pda(payer, config_account, system, program_id, &seeds, CONFIG_SIZE)?;
    Config {
        coin_mint: *coin_mint.key,
        distribution_program: *distribution_program.key,
        distribution_config: *distribution_config.key,
        percolator_program: *percolator_program.key,
        total_supply,
        fee_support_bps: 0,
        emission_end_slot,
        total_points: 0, // BACKING cohort
        sealed: 0,
        bump,
        insurance_bps,
        insurance_total_points: 0,
        subledger_program: *subledger_program.key,
        subledger_pool,
        market_group,
        frozen_total_points: 0,
        frozen_insurance_total_points: 0,
        freeze_slot: 0,
        vault: Pubkey::default(),
        finalize_window,
        backing_pool,
        backing_bps,
        lp_bps,
        lp_total_points: 0,
        trader_total_points: 0,
        frozen_lp_total_points: 0,
        frozen_trader_total_points: 0,
    }
    .serialize(&mut config_account.try_borrow_mut_data()?);
    Ok(())
}

// register_start accounts: [payer(s,w), config, owner, recipient, linked, stake(pda,w), system]
//   residual:  linked = percolator backing ledger; insurance: linked = subledger position.
// data: cohort(u8)
fn register_start(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let cohort = *data.first().ok_or(ProgramError::InvalidInstructionData)?;
    if cohort > COHORT_TRADER {
        return Err(ProgramError::InvalidInstructionData);
    }
    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let config_account = next_account_info(iter)?;
    let owner = next_account_info(iter)?;
    let recipient = next_account_info(iter)?;
    let linked = next_account_info(iter)?;
    let stake_account = next_account_info(iter)?;
    let system = next_account_info(iter)?;

    // `owner` must SIGN: registering binds this stake's COIN recipient, a privileged act only the
    // rightful party may authorize. Without it, anyone could front-run the victim's (per-owner)
    // stake PDA naming themselves recipient, permanently denying the victim their share (finding GY).
    if !payer.is_signer || !owner.is_signer || config_account.owner != program_id {
        return Err(if config_account.owner != program_id {
            ProgramError::IllegalOwner
        } else {
            ProgramError::MissingRequiredSignature
        });
    }
    // The COIN recipient must be a real key (finding IK): a default-pubkey recipient is never legitimate,
    // and a crystallized stake bound to it can NEVER be sealed — distribution::append rejects a
    // default-pubkey entry, yet HD/HX completeness require every crystallized stake represented, so one
    // such (active) stake makes the seal permanently unsatisfiable = a single-stake DOS on any genesis.
    if *recipient.key == Pubkey::default() {
        return Err(ProgramError::InvalidArgument);
    }
    let config = Config::deserialize(&config_account.try_borrow_data()?)?;
    if config.freeze_slot != 0 {
        return Err(ProgramError::InvalidAccountData); // denominators frozen — no new registrations
    }
    let now = Clock::get()?.slot;
    // `snap` is the register-time counter snapshot: 0 for the share-value cohorts (insurance/backing —
    // points are the LIVE shares read at crystallize/claim), and the portfolio residual counter for the
    // LP/trader cohorts (Δ measured at crystallize).
    let snap: u128 = match cohort {
        COHORT_INSURANCE | COHORT_BACKING => {
            // Share-value cohort: `linked` is a subledger Position in this cohort's pool.
            if *linked.owner != config.subledger_program {
                return Err(ProgramError::IllegalOwner);
            }
            let data = linked.try_borrow_data()?;
            // Bind the position to its depositor (finding GY): only the rightful owner may register it.
            if pk(&data, SUB_POS_OWNER) != *owner.key {
                return Err(ProgramError::IllegalOwner);
            }
            // Scope to THIS genesis's pool (finding HG): insurance -> subledger_pool, backing -> backing_pool.
            let scope_pool = if cohort == COHORT_INSURANCE {
                config.subledger_pool
            } else {
                config.backing_pool
            };
            if pk(&data, SUB_POS_POOL) != scope_pool {
                return Err(ProgramError::IllegalOwner);
            }
            0
        }
        _ => {
            // Residual cohort (LP/trader): `linked` is a percolator PortfolioAccount. (Scope is
            // account-level — the residual counters are per-account totals; the genesis is single-market,
            // so account-level == this-market. market_group is informational here.)
            if *linked.owner != config.percolator_program {
                return Err(ProgramError::IllegalOwner); // counters must be percolator-authenticated
            }
            let data = linked.try_borrow_data()?;
            // Bind the portfolio to its owner (finding GY).
            if pk(&data, OFF_PORTFOLIO_OWNER) != *owner.key {
                return Err(ProgramError::IllegalOwner);
            }
            // Scope to the ONE allow-listed (trusted-Pyth) genesis market (finding IL). The residual
            // counters are wash-manufacturable on a market whose oracle the registrant controls, so a
            // portfolio only counts if it belongs to config.market_group — the market the orchestrator
            // vetted as Pyth-priced at init. An attacker's own auth-mark market is rejected here.
            if pk(&data, OFF_PORTFOLIO_MARKET_GROUP) != config.market_group {
                return Err(ProgramError::IllegalOwner);
            }
            let (received, crystallized) = read_portfolio_residual(&data)?;
            if cohort == COHORT_LP {
                received
            } else {
                crystallized
            }
        }
    };
    let start_slot = now;
    let (expected, bump) = Pubkey::find_program_address(
        &[b"rd_stake", config_account.key.as_ref(), owner.key.as_ref()],
        program_id,
    );
    if *stake_account.key != expected || stake_account.data_len() != 0 {
        return Err(ProgramError::InvalidSeeds);
    }
    let bump_arr = [bump];
    let seeds: [&[u8]; 4] = [b"rd_stake", config_account.key.as_ref(), owner.key.as_ref(), &bump_arr];
    create_pda(payer, stake_account, system, program_id, &seeds, STAKE_SIZE)?;
    Stake {
        config: *config_account.key,
        owner: *owner.key,
        backing_ledger: *linked.key,
        recipient: *recipient.key,
        residual_snap: snap,
        earnings_snap: 0,
        start_slot,
        points: 0,
        bump,
        cohort,
        eligible_accum: 0,
        claimed: false,
    }
    .serialize(&mut stake_account.try_borrow_mut_data()?);
    Ok(())
}

// crystallize accounts: [cranker(s), config(w), stake(w), backing_ledger]
fn crystallize(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let iter = &mut accounts.iter();
    let cranker = next_account_info(iter)?;
    let config_account = next_account_info(iter)?;
    let stake_account = next_account_info(iter)?;
    let backing_ledger = next_account_info(iter)?;

    if !cranker.is_signer
        || config_account.owner != program_id
        || stake_account.owner != program_id
    {
        return Err(ProgramError::IllegalOwner);
    }
    let mut config = Config::deserialize(&config_account.try_borrow_data()?)?;
    if config.sealed != 0 || config.freeze_slot != 0 {
        return Err(ProgramError::InvalidAccountData); // sealed or frozen -> denominators are final
    }
    let mut stake = Stake::deserialize(&stake_account.try_borrow_data()?)?;
    if stake.config != *config_account.key || stake.backing_ledger != *backing_ledger.key {
        return Err(ProgramError::InvalidAccountData);
    }
    // subtract-old/add-new keeps the cohort denominator authoritative as points are re-derived. The
    // claim-time live cap (insurance/backing: live shares; not needed for LP/trader monotonic counters)
    // is the backstop against any stale-high denominator contribution.
    match stake.cohort {
        COHORT_INSURANCE | COHORT_BACKING => {
            // Share-value cohort: points = LIVE Position.shares (0 if exited — soft veto). The share
            // price is common within the pool, so shares == pro-rata share value, fee-weighted.
            // OWNER-GATED (finding KO, KM parity): crystallize OVERWRITES stake.points from the live
            // shares NOW, and freeze then locks that value as the frozen denominator term — which the
            // claim-time min-cap can only ever LOWER, never raise. So a permissionless caller could
            // force-crystallize a victim at a transient low-share moment (mid partial-withdraw:
            // withdrawn=false, shares reduced) and `freeze` to lock the victim's COIN share permanently
            // low. A share-value re-crystallize must therefore be authorized by the stake's own owner.
            // (LP/trader stay permissionless — their counters are monotonic, so a forced crystallize can
            // only raise the Δ, never grief.)
            if cranker.key != &stake.owner {
                return Err(ProgramError::MissingRequiredSignature);
            }
            if *backing_ledger.owner != config.subledger_program {
                return Err(ProgramError::IllegalOwner);
            }
            let (shares, withdrawn) = read_subledger_shares(&backing_ledger.try_borrow_data()?)?;
            let new_pts = share_value_points(shares, withdrawn);
            let slot = config.cohort_points_mut(stake.cohort);
            *slot = slot.saturating_sub(stake.points).saturating_add(new_pts);
            stake.points = new_pts;
        }
        _ => {
            // Residual cohort (LP/trader): points = Δ(counter) since register. The counters are monotonic
            // and backed by REAL crystallized loss (spent<=crystallized, shape-validated by percolator),
            // so they can't be farmed without actually losing money — no fee/JIT cap needed.
            if *backing_ledger.owner != config.percolator_program {
                return Err(ProgramError::IllegalOwner);
            }
            let (received, crystallized) = read_portfolio_residual(&backing_ledger.try_borrow_data()?)?;
            let counter = if stake.cohort == COHORT_LP { received } else { crystallized };
            let new_pts = counter.saturating_sub(stake.residual_snap);
            let slot = config.cohort_points_mut(stake.cohort);
            *slot = slot.saturating_sub(stake.points).saturating_add(new_pts);
            stake.points = new_pts;
        }
    }

    stake.serialize(&mut stake_account.try_borrow_mut_data()?);
    config.serialize(&mut config_account.try_borrow_mut_data()?);
    Ok(())
}

// freeze accounts: [cranker(s), config(w), coin_mint, vault]
//
// Permissionless. After emission_end, this is the one-shot transition from the accrual phase
// (register/crystallize) to the self-service claim phase. It (1) snapshots the cohort denominators
// (total_points, insurance_total_points) and stamps freeze_slot, after which register/crystallize are
// closed so the denominators are final; and (2) BINDS + verifies the COIN vault claims pay from: it
// must be a token account OWNED BY this rd_config PDA, holding the full fixed supply (EZ), with the
// coin_mint carrying NO mint or freeze authority (GX) so the supply can't be inflated or frozen under
// the claimers. double-freeze is rejected so neither the snapshot nor the vault can be moved.
fn freeze(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let iter = &mut accounts.iter();
    let cranker = next_account_info(iter)?;
    let config_account = next_account_info(iter)?;
    let coin_mint = next_account_info(iter)?;
    let vault = next_account_info(iter)?;
    if !cranker.is_signer || config_account.owner != program_id {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let mut config = Config::deserialize(&config_account.try_borrow_data()?)?;
    if config.freeze_slot != 0 {
        return Err(ProgramError::InvalidAccountData); // already frozen — snapshot + vault are immutable
    }
    let now = Clock::get()?.slot;
    if now < config.emission_end_slot.saturating_add(config.finalize_window) {
        return Err(ProgramError::InvalidInstructionData); // emission + finalize window still open
    }
    // GX: the COIN is a fixed pool — no mint authority (can't inflate) and no freeze authority (can't
    // freeze a claimer's account). EZ: the bound vault is rd_config-owned and holds the WHOLE supply.
    if *coin_mint.key != config.coin_mint {
        return Err(ProgramError::InvalidAccountData);
    }
    let mint = spl_token::state::Mint::unpack(&coin_mint.try_borrow_data()?)?;
    if mint.mint_authority.is_some() || mint.freeze_authority.is_some() || mint.supply != config.total_supply {
        return Err(ProgramError::InvalidAccountData);
    }
    let v = spl_token::state::Account::unpack(&vault.try_borrow_data()?)?;
    // owner == rd_config + funded with the whole supply (EZ). No delegate/close_authority check is
    // needed: SPL's set_authority(AccountOwner) clears delegate + delegated_amount + close_authority,
    // and rd_config (a PDA with no approve instruction) can never set them — so a vault handed to
    // rd_config is SOLELY rd-controlled. The freeze vault/mint guards (owner, full funding, no mint/freeze
    // authority) are pinned by the e2e test `freeze_enforces_fixed_supply_and_vault_integrity`.
    if v.owner != *config_account.key || v.mint != config.coin_mint || v.amount < config.total_supply {
        return Err(ProgramError::InvalidAccountData);
    }
    config.vault = *vault.key;
    // Snapshot all four cohort denominators.
    config.frozen_insurance_total_points = config.insurance_total_points;
    config.frozen_total_points = config.total_points; // BACKING
    config.frozen_lp_total_points = config.lp_total_points;
    config.frozen_trader_total_points = config.trader_total_points;
    config.freeze_slot = now;
    config.serialize(&mut config_account.try_borrow_mut_data()?);
    Ok(())
}

// claim accounts: [cranker(s), config, stake(w), vault(w), recipient_ata(w), token_program]
//   insurance cohort appends one more: the subledger position (for the live HE cap).
//
// PERMISSIONLESS self-service residual claim (replaces the cranker-assembled seal for the residual
// cohort). Pays the stake's OWN deterministic share —
// `residual_supply * stake.points / frozen_total_points` — to the stake's BOUND recipient, then marks
// it claimed. Each backer pulls their own slice; nobody assembles a global list, so there is no
// one-tx completeness seal (IG dissolved) and no cranker can omit or redirect a backer (the recipient
// is bound at register, finding GY, and re-checked here). Sum of all residual claims <= residual_supply
// (floor math), so the vault can never be over-drawn. The residual cohort uses the crystallized,
// now-frozen `stake.points` (cumulative loss — only ever grows), so there is no live-position
// dependency and no HE concern; the insurance cohort (live, HE-capped) is handled separately.
fn claim(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let iter = &mut accounts.iter();
    let cranker = next_account_info(iter)?;
    let config_account = next_account_info(iter)?;
    let stake_account = next_account_info(iter)?;
    let vault = next_account_info(iter)?;
    let recipient_ata = next_account_info(iter)?;
    let token_program = next_account_info(iter)?;
    if !cranker.is_signer || config_account.owner != program_id || stake_account.owner != program_id {
        return Err(ProgramError::MissingRequiredSignature);
    }
    // Pin token_program to the real SPL token program (defense-in-depth, matching distribution:619). A
    // substituted token_program is ALREADY rejected by spl_token::instruction::transfer's internal
    // check_program_account (propagated by `?` below, BEFORE any foreign program is invoked), so the
    // "no-op program nullifies a claim" grief is blocked regardless; this explicit guard makes the
    // invariant local + survives a future refactor to a hand-built transfer instruction (finding KE).
    if *token_program.key != spl_token::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    let config = Config::deserialize(&config_account.try_borrow_data()?)?;
    if config.freeze_slot == 0 {
        return Err(ProgramError::InvalidAccountData); // not frozen -> denominators not final
    }
    if *vault.key != config.vault {
        return Err(ProgramError::InvalidAccountData); // only the bound funded vault — no decoy
    }
    let mut stake = Stake::deserialize(&stake_account.try_borrow_data()?)?;
    if stake.config != *config_account.key {
        return Err(ProgramError::InvalidAccountData);
    }
    if stake.claimed {
        return Err(ProgramError::InvalidAccountData); // double-claim
    }
    // Share-value cohorts (insurance/backing) cap the payout by LIVE shares at claim time, so the claim
    // SLOT is value-relevant. claim is otherwise permissionless; if a third party could trigger a
    // share-value claim they could force it during a transient low-share moment — e.g. mid partial
    // insurance-withdraw, which leaves the Position withdrawn=false but shares reduced — and the
    // irreversible claimed-flag would lock in the reduced (or zero) payout, stranding the remainder
    // (finding KM). So a share-value claim must be authorized by the stake's OWN owner (the depositor who
    // controls the shares and bears the soft-veto timing). LP/trader pay frozen points with no live cap,
    // so their claim slot is irrelevant and stays permissionless (any cranker may finalize them).
    if matches!(stake.cohort, COHORT_INSURANCE | COHORT_BACKING) && cranker.key != &stake.owner {
        return Err(ProgramError::MissingRequiredSignature);
    }
    // The COIN must land in the bound recipient's own account (finding GY: no cranker redirect).
    let ra = spl_token::state::Account::unpack(&recipient_ata.try_borrow_data()?)?;
    if ra.owner != stake.recipient || ra.mint != config.coin_mint {
        return Err(ProgramError::InvalidAccountData);
    }
    let cohort_supply = config.cohort_supply(stake.cohort);
    let frozen_denom = config.frozen_cohort_points(stake.cohort);
    let amount = match stake.cohort {
        COHORT_INSURANCE | COHORT_BACKING => {
            // Share-value cohort: read the LIVE Position shares NOW and cap by them ATOMICALLY (finding
            // HE/JC + soft veto). A depositor who redeemed shares after freeze -> fewer live shares ->
            // claims less; a full exit -> 0 shares -> 0 COIN (forfeit). The appended account is the
            // bound subledger position. read+cap+pay in ONE tx, so there is no finalize/claim over-claim gap.
            let position = next_account_info(iter)?;
            if *position.key != stake.backing_ledger || *position.owner != config.subledger_program {
                return Err(ProgramError::InvalidAccountData);
            }
            let (shares, withdrawn) = read_subledger_shares(&position.try_borrow_data()?)?;
            let live_pts = share_value_points(shares, withdrawn);
            let pts = if stake.points < live_pts { stake.points } else { live_pts };
            points_to_amount(cohort_supply, pts, frozen_denom)
        }
        _ => {
            // Residual cohort (LP/trader): frozen monotonic Δ — no live dependency, no cap account needed.
            points_to_amount(cohort_supply, stake.points, frozen_denom)
        }
    };
    // Mark claimed before paying (the whole tx reverts on a transfer failure, so this is atomic).
    stake.claimed = true;
    stake.serialize(&mut stake_account.try_borrow_mut_data()?);
    if amount > 0 {
        let bump_arr = [config.bump];
        let signer_seeds: [&[u8]; 3] = [b"rd_config", config.coin_mint.as_ref(), &bump_arr];
        invoke_signed(
            &spl_token::instruction::transfer(
                token_program.key,
                vault.key,
                recipient_ata.key,
                config_account.key,
                &[],
                amount,
            )?,
            &[
                vault.clone(),
                recipient_ata.clone(),
                config_account.clone(),
                token_program.clone(),
            ],
            &[&signer_seeds],
        )?;
    }
    Ok(())
}

fn take_u64(data: &mut &[u8]) -> Result<u64, ProgramError> {
    let b = data.get(..8).ok_or(ProgramError::InvalidInstructionData)?;
    *data = &data[8..];
    Ok(u64::from_le_bytes(b.try_into().unwrap()))
}
fn take_u16(data: &mut &[u8]) -> Result<u16, ProgramError> {
    let b = data.get(..2).ok_or(ProgramError::InvalidInstructionData)?;
    *data = &data[2..];
    Ok(u16::from_le_bytes(b.try_into().unwrap()))
}
fn take_pubkey(data: &mut &[u8]) -> Result<Pubkey, ProgramError> {
    let b = data.get(..32).ok_or(ProgramError::InvalidInstructionData)?;
    *data = &data[32..];
    Ok(Pubkey::new_from_array(b.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_is_pro_rata_and_never_over_allocates() {
        assert_eq!(points_to_amount(1_000_000, 30, 100), 300_000);
        assert_eq!(points_to_amount(1_000_000, 70, 100), 700_000);
        assert!(points_to_amount(1_000_000, 30, 100) + points_to_amount(1_000_000, 70, 100) <= 1_000_000);
        assert_eq!(points_to_amount(1_000_000, 1, 0), 0);
    }

    #[test]
    fn reads_live_subledger_shares_offsets() {
        let mut d = [0u8; 120];
        d[88] = 1; // withdrawn
        d[104..120].copy_from_slice(&777u128.to_le_bytes()); // shares
        let (shares, w) = read_subledger_shares(&d).unwrap();
        assert_eq!(shares, 777);
        assert!(w);
        assert_eq!(share_value_points(777, true), 0, "withdrawn -> forfeit");
        assert_eq!(share_value_points(777, false), 777, "live -> shares");
    }
}
