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
use alloc::vec;
use alloc::vec::Vec;

use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
    declare_id,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
    program_error::ProgramError,
    program_pack::Pack,
    pubkey::Pubkey,
    rent::Rent,
    system_instruction,
    sysvar::Sysvar,
};

declare_id!("Res1dua1Distr1butor111111111111111111111111");

const DIST_IX_SEAL_WINNER: u8 = 3;
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
const CONFIG_SIZE: usize = 366; // +32 vault +8 finalize_window (self-service)
const STAKE_SIZE: usize = 211; // +1 claimed flag (self-service)
const COHORT_RESIDUAL: u8 = 0;
const COHORT_INSURANCE: u8 = 1;

// distribution proposal byte layout (must track distribution/src/lib.rs).
const DIST_PROPOSAL_HEADER: usize = 104;
const DIST_ENTRY_SIZE: usize = 40;
const DIST_HDR_ENTRY_COUNT_OFF: usize = 84; // u32 entry_count in ProposalHeader (config@8,id@40,creator@48,capacity@80,entry_count@84)

const IX_INIT: u8 = 0;
const IX_REGISTER_START: u8 = 1;
const IX_CRYSTALLIZE: u8 = 2;
const IX_SEAL: u8 = 3;
// Self-service path (replacing the cranker seal). After emission_end, IX_FREEZE snapshots the
// cohort denominators and closes register/crystallize; backers then finalize/claim their own share.
const IX_FREEZE: u8 = 4;
const IX_CLAIM: u8 = 5;

// ===========================================================================
// Deterministic, gaming-resistant point math  (pure — unit-tested below)
// ===========================================================================

/// `floor(log2(n))`, 0 for n < 2. Deliberately weak: doubling the hold adds one step.
#[inline]
pub fn floor_log2(n: u64) -> u32 {
    if n < 2 {
        0
    } else {
        63 - n.leading_zeros()
    }
}

/// Wash cap: eligible backing capped by sunk fee revenue. No fees ⇒ 0.
pub fn fee_supported_eligible(residual_delta: u128, fee_delta: u128, fee_support_bps: u16) -> u128 {
    if fee_support_bps == 0 || residual_delta == 0 || fee_delta == 0 {
        return 0;
    }
    let cap = fee_delta.saturating_mul(BPS_DENOMINATOR as u128) / fee_support_bps as u128;
    if residual_delta < cap {
        residual_delta
    } else {
        cap
    }
}

/// Points for one window: `eligible * floor(log2(hold))`. JIT (tiny hold) ⇒ ~0.
pub fn window_points(
    residual_delta: u128,
    fee_delta: u128,
    fee_support_bps: u16,
    hold_slots: u64,
) -> u128 {
    fee_supported_eligible(residual_delta, fee_delta, fee_support_bps)
        .saturating_mul(floor_log2(hold_slots) as u128)
}

/// Deterministic pro-rata split; floor rounding never over-allocates the fixed pool.
pub fn points_to_amount(total_supply: u64, points_i: u128, total_points: u128) -> u64 {
    if total_points == 0 {
        return 0;
    }
    ((total_supply as u128).saturating_mul(points_i) / total_points) as u64
}

// ===========================================================================
// percolator BackingDomainLedgerAccountV16 snapshot read (offsets PINNED)
// ===========================================================================
// Account = HEADER_LEN(16) + repr(C) struct {market_group[32], authority[32],
// total_principal u128@64, total_deposited, total_principal_withdrawn,
// total_earnings u128@112, total_earnings_withdrawn, last_observed_bucket,
// cumulative_loss u128@160, ...}. Absolute = 16 + within-struct. PINNED against the
// real struct by `tests/offsets.rs` (offset_of! + HEADER_LEN), finding-T discipline —
// the meta program's un-pinned offsets (finding GT) are the cautionary tale.
pub const PERC_HEADER_LEN: usize = 16;
// authority @ HEADER_LEN + offset_of(market_group[32]) = 48 — the LP that owns this backing
// ledger and is owed its residual COIN reward. PINNED by tests/offsets.rs.
// market_group @ HEADER_LEN + 0 — which percolator market this backing ledger belongs to. Used to
// scope the residual cohort to the genesis market (finding HI). PINNED by tests/offsets.rs.
pub const OFF_BACKING_MARKET_GROUP: usize = PERC_HEADER_LEN;
pub const OFF_BACKING_AUTHORITY: usize = PERC_HEADER_LEN + 32;
pub const OFF_TOTAL_PRINCIPAL: usize = PERC_HEADER_LEN + 64;
pub const OFF_TOTAL_EARNINGS: usize = PERC_HEADER_LEN + 112;
pub const OFF_CUMULATIVE_LOSS: usize = PERC_HEADER_LEN + 160;

fn read_u128(data: &[u8], off: usize) -> Result<u128, ProgramError> {
    let b = data.get(off..off + 16).ok_or(ProgramError::AccountDataTooSmall)?;
    Ok(u128::from_le_bytes(b.try_into().unwrap()))
}

/// (residual_received, total_earnings, total_principal). residual_received = cumulative_loss_atoms.
pub fn read_backing_counters(data: &[u8]) -> Result<(u128, u128, u128), ProgramError> {
    Ok((
        read_u128(data, OFF_CUMULATIVE_LOSS)?,
        read_u128(data, OFF_TOTAL_EARNINGS)?,
        read_u128(data, OFF_TOTAL_PRINCIPAL)?,
    ))
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
pub const SUB_POS_PRINCIPAL: usize = 72;
pub const SUB_POS_WITHDRAWN: usize = 88;
pub const SUB_POS_START_SLOT: usize = 89;
// Position.shares (POLICY_WITH_SURPLUS) @104 — the SHARE-VALUE points source for the insurance AND
// backing cohorts. Within one pool the share price (balance/total_shares) is common, so pro-rata by
// share value == pro-rata by shares; shares also encode the fee/time weighting (an earlier depositor
// holds more shares per dollar) and give the soft-veto for free (exit redeems shares -> 0 -> forfeit).
pub const SUB_POS_SHARES: usize = 104;

/// (principal, start_slot, withdrawn) from a live subledger Position account.
pub fn read_subledger_position(data: &[u8]) -> Result<(u64, u64, bool), ProgramError> {
    let principal = u64::from_le_bytes(
        data.get(SUB_POS_PRINCIPAL..SUB_POS_PRINCIPAL + 8)
            .ok_or(ProgramError::AccountDataTooSmall)?
            .try_into()
            .unwrap(),
    );
    let withdrawn = *data.get(SUB_POS_WITHDRAWN).ok_or(ProgramError::AccountDataTooSmall)? == 1;
    let start_slot = u64::from_le_bytes(
        data.get(SUB_POS_START_SLOT..SUB_POS_START_SLOT + 8)
            .ok_or(ProgramError::AccountDataTooSmall)?
            .try_into()
            .unwrap(),
    );
    Ok((principal, start_slot, withdrawn))
}

/// Insurance-cohort COIN points = `principal * floor(log2(seal_slot - start_slot))`,
/// computed LIVE at seal. A withdrawn (or zero-principal) position yields 0 — the soft
/// veto: a depositor who exited with the surplus has forfeited its COIN share, and that
/// share is simply never allocated (burned as unclaimed by distribution::burn_unclaimed).
pub fn insurance_points(seal_slot: u64, principal: u64, start_slot: u64, withdrawn: bool) -> u128 {
    if withdrawn || principal == 0 {
        return 0;
    }
    (principal as u128).saturating_mul(floor_log2(seal_slot.saturating_sub(start_slot)) as u128)
}

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
        IX_SEAL => seal(program_id, accounts),
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

    let total_supply = take_u64(&mut data)?;
    let fee_support_bps = take_u16(&mut data)?;
    let emission_end_slot = take_u64(&mut data)?;
    let insurance_bps = take_u16(&mut data)?;
    // The genesis insurance pool the insurance cohort is scoped to (finding HG). Optional in the
    // wire format (residual-only genesis omits it); REQUIRED whenever insurance_bps > 0.
    let subledger_pool = if data.len() >= 32 {
        let p = Pubkey::new_from_array(data[..32].try_into().unwrap());
        data = &data[32..];
        p
    } else {
        Pubkey::default()
    };
    // The genesis market_group the residual cohort is scoped to (finding HI). Optional in the wire
    // format; when set (non-default), register_start requires residual ledgers to match it.
    let market_group = if data.len() >= 32 {
        let m = Pubkey::new_from_array(data[..32].try_into().unwrap());
        data = &data[32..];
        m
    } else {
        Pubkey::default()
    };
    // Trailing optional: the post-emission finalize window (slots) before freeze is allowed. The
    // orchestrator sets ~1 week so slower backers can do their final crystallize before the
    // permissionless freeze locks the denominators (a 0 window leaves a premature-freeze grief).
    let finalize_window = if data.len() >= 8 {
        let w = u64::from_le_bytes(data[..8].try_into().unwrap());
        data = &data[8..];
        w
    } else {
        0
    };
    if !data.is_empty() || !payer.is_signer || total_supply == 0 || fee_support_bps == 0 || insurance_bps > BPS_DENOMINATOR as u16 {
        return Err(ProgramError::InvalidInstructionData);
    }
    // An insurance cohort MUST be scoped to a concrete pool: otherwise an insurance position from any
    // other pool of the same subledger program could be registered to farm this genesis's COIN.
    if insurance_bps > 0 && subledger_pool == Pubkey::default() {
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
        fee_support_bps,
        emission_end_slot,
        total_points: 0,
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
    }
    .serialize(&mut config_account.try_borrow_mut_data()?);
    Ok(())
}

// register_start accounts: [payer(s,w), config, owner, recipient, linked, stake(pda,w), system]
//   residual:  linked = percolator backing ledger; insurance: linked = subledger position.
// data: cohort(u8)
fn register_start(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let cohort = *data.first().ok_or(ProgramError::InvalidInstructionData)?;
    if cohort > COHORT_INSURANCE {
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
    let (residual, earnings, start_slot) = if cohort == COHORT_RESIDUAL {
        if *linked.owner != config.percolator_program {
            return Err(ProgramError::IllegalOwner); // counters must be percolator-authenticated
        }
        let data = linked.try_borrow_data()?;
        // The backing ledger's COIN reward is owed to its `authority` (the LP that absorbed the
        // loss). Bind it to `owner` so an attacker cannot farm a victim's ledger (finding GY).
        if pk(&data, OFF_BACKING_AUTHORITY) != *owner.key {
            return Err(ProgramError::IllegalOwner);
        }
        // Scope to the genesis market (findings HI/HN): a backing ledger from any OTHER percolator
        // market must not farm this genesis's residual COIN. UNCONDITIONAL (fail-closed): if the
        // orchestrator forgets to set config.market_group, real backing ledgers (non-zero market_group)
        // are REJECTED (residual register DOS) rather than left unscoped (silent cross-market farming),
        // forcing correct scoping. A real percolator market_group is never the default.
        if pk(&data, OFF_BACKING_MARKET_GROUP) != config.market_group {
            return Err(ProgramError::IllegalOwner);
        }
        let (r, e, _p) = read_backing_counters(&data)?;
        (r, e, Clock::get()?.slot)
    } else {
        // insurance: linked is a subledger position; tenure starts at the position's own deposit slot.
        if *linked.owner != config.subledger_program {
            return Err(ProgramError::IllegalOwner);
        }
        let data = linked.try_borrow_data()?;
        // The position's COIN is owed to its depositor (`Position.owner`); bind it to `owner`.
        if pk(&data, SUB_POS_OWNER) != *owner.key {
            return Err(ProgramError::IllegalOwner);
        }
        // Scope to the ONE genesis insurance pool (finding HG): a position from any other pool of the
        // same subledger program must not farm this genesis's insurance COIN.
        if pk(&data, SUB_POS_POOL) != config.subledger_pool {
            return Err(ProgramError::IllegalOwner);
        }
        let (_principal, pos_start, _withdrawn) = read_subledger_position(&data)?;
        (0, 0, pos_start)
    };
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
        residual_snap: residual,
        earnings_snap: earnings,
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
    let now = Clock::get()?.slot;
    if stake.cohort == COHORT_RESIDUAL {
        if *backing_ledger.owner != config.percolator_program {
            return Err(ProgramError::IllegalOwner);
        }
        let (residual, earnings, _principal) = read_backing_counters(&backing_ledger.try_borrow_data()?)?;
        // RE-DERIVE from REGISTER-to-now TOTALS (snapshots are NEVER advanced — they hold the values
        // captured at register). Points = fee_supported_eligible(total residual, total fees) *
        // floor_log2(true tenure). This makes points depend ONLY on (total eligible since register,
        // true tenure) — independent of crystallize timing/count, defeating two permissionless griefs:
        //   - GZ: chopping tenure into tiny floor_log2 windows (the multiplier now uses the original
        //     start_slot against the running total, so a late crystallize fully recovers); and
        //   - HO: crystallizing in a fees-lagging window (Δfee=0 per-window -> eligible 0) to consume
        //     residual for nothing — the fee cap now applies to the WHOLE-period totals, which align
        //     regardless of when residual vs fees synced.
        // saturating_sub guards a counter reset. subtract-old/add-new keeps config.total_points
        // authoritative as the re-derived points grow.
        let total_res = residual.saturating_sub(stake.residual_snap);
        let total_fee = earnings.saturating_sub(stake.earnings_snap);
        let elig = fee_supported_eligible(total_res, total_fee, config.fee_support_bps);
        let hold = now.saturating_sub(stake.start_slot);
        let new_pts = elig.saturating_mul(floor_log2(hold) as u128);
        config.total_points = config.total_points.saturating_sub(stake.points).saturating_add(new_pts);
        stake.points = new_pts;
    } else {
        // insurance: re-derive the LEVEL (capital*log-time) from the LIVE position and keep the
        // authoritative total via subtract-old/add-new.
        if *backing_ledger.owner != config.subledger_program {
            return Err(ProgramError::IllegalOwner);
        }
        let (principal, pos_start, withdrawn) = read_subledger_position(&backing_ledger.try_borrow_data()?)?;
        // A withdrawn position is a NO-OP (finding HR): its crystallized points STAY in
        // insurance_total_points so the forfeited share is BURNED (unclaimed -> burn), not
        // REDISTRIBUTED to the survivors by a permissionless re-crystallize that drops it from the
        // denominator. seal independently forces a withdrawn entry to amount 0, so the points are never
        // paid out — they only hold the denominator so the share is burned deterministically.
        if !withdrawn {
            let new_pts = insurance_points(now, principal, pos_start, withdrawn);
            config.insurance_total_points = config
                .insurance_total_points
                .saturating_sub(stake.points)
                .saturating_add(new_pts);
            stake.points = new_pts;
        }
    }

    stake.serialize(&mut stake_account.try_borrow_mut_data()?);
    config.serialize(&mut config_account.try_borrow_mut_data()?);
    Ok(())
}

// seal accounts: [cranker(s), config(w), distribution_program, distribution_config(w),
//   proposal(w), <stake_0, stake_1, ... stake_{entry_count-1}>, <forfeited insurance extras>]
// Verifies each proposal entry == the deterministic (recipient, amount) from its stake,
// then CPIs distribution::seal_winner signed by the config PDA.
//
// ADVERSARIAL LIMITATION (finding IG): the HD/HX completeness checks require EVERY crystallized stake
// to be represented in THIS one seal tx (each insurance stake costs 2 accounts: stake + position), so
// at most ~61 insurance stakes fit under Solana's 128 account-lock cap. Because register_start +
// crystallize are permissionless and unbounded, an attacker can Sybil-flood dust stakes past that cap
// and make completeness unsatisfiable -> a permanent seal DOS (no fund theft; GY/HC/HK still hold).
// This decider is therefore intended for TRUSTED-CRANKER / BOUNDED-PARTICIPANT genesis. For a genesis
// that must be robust to an adversarial flood, use the genesis-vote decider instead: its `trigger` is
// O(1) (fixed accounts; voters are tracked in running tallies, not iterated at seal). A future
// hardening (chunked accumulate+finalize seal, with a per-stake counted flag and crystallize gated to
// emission_end so total_points is frozen) would make this decider adversary-robust.
fn seal(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let iter = &mut accounts.iter();
    let cranker = next_account_info(iter)?;
    let config_account = next_account_info(iter)?;
    let distribution_program = next_account_info(iter)?;
    let distribution_config = next_account_info(iter)?;
    let proposal = next_account_info(iter)?;

    if !cranker.is_signer || config_account.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    let mut config = Config::deserialize(&config_account.try_borrow_data()?)?;
    if config.sealed != 0
        || *distribution_program.key != config.distribution_program
        || *distribution_config.key != config.distribution_config
    {
        return Err(ProgramError::InvalidAccountData);
    }
    // Defense-in-depth (finding HW): the proposal's bytes are read RAW for the per-entry verification
    // below, so require it to actually be a distribution-owned account — every other account in seal
    // is owner/key-checked. (seal_winner re-validates proposal.config at the CPI, so this is a clean
    // early reject + avoids interpreting an arbitrary foreign account's bytes as proposal entries.)
    if *proposal.owner != config.distribution_program {
        return Err(ProgramError::IllegalOwner);
    }
    let seal_slot = Clock::get()?.slot;
    if seal_slot < config.emission_end_slot {
        return Err(ProgramError::InvalidInstructionData); // emission still open
    }

    // Supply split: insurance cohort gets `insurance_bps`, residual-backers the rest.
    let insurance_supply =
        ((config.total_supply as u128) * config.insurance_bps as u128 / BPS_DENOMINATOR as u128) as u64;
    let residual_supply = config.total_supply - insurance_supply;

    // Re-derive every entry from its on-chain stake and require an exact match. Each entry is
    // accompanied by its stake; an INSURANCE stake is also accompanied by its live subledger
    // position so the seal can FORFEIT (amount must be 0) a depositor that has exited.
    let pd = proposal.try_borrow_data()?;
    let entry_count = u32::from_le_bytes(
        pd.get(DIST_HDR_ENTRY_COUNT_OFF..DIST_HDR_ENTRY_COUNT_OFF + 4)
            .ok_or(ProgramError::AccountDataTooSmall)?
            .try_into()
            .unwrap(),
    ) as usize;
    // Each stake may back AT MOST ONE entry. distribution::claim is per-index (a recipient at two
    // indices claims both), so without this a cranker could duplicate a high-value stake within the
    // supply headroom left by omitting other stakes and double-claim it -> over-allocation theft
    // (finding HA). entry_count is bounded by the tx account limit, so an O(n^2) seen-check is fine.
    let mut seen: Vec<Pubkey> = Vec::with_capacity(entry_count);
    let mut sealed_residual_points: u128 = 0;
    let mut sealed_insurance_points: u128 = 0;
    for i in 0..entry_count {
        let stake_account = next_account_info(iter)?;
        if stake_account.owner != program_id {
            return Err(ProgramError::IllegalOwner);
        }
        if seen.contains(stake_account.key) {
            return Err(ProgramError::InvalidAccountData); // a stake cannot back two entries
        }
        seen.push(*stake_account.key);
        let stake = Stake::deserialize(&stake_account.try_borrow_data()?)?;
        if stake.config != *config_account.key {
            return Err(ProgramError::InvalidAccountData);
        }
        let want = if stake.cohort == COHORT_RESIDUAL {
            sealed_residual_points = sealed_residual_points.saturating_add(stake.points);
            points_to_amount(residual_supply, stake.points, config.total_points)
        } else {
            // Insurance: read the LIVE position; a withdrawn depositor forfeits (amount 0). The
            // forfeited crystallized share stays in insurance_total_points -> burned as unclaimed.
            let position = next_account_info(iter)?;
            if *position.key != stake.backing_ledger || *position.owner != config.subledger_program {
                return Err(ProgramError::InvalidAccountData);
            }
            // Count this entry's crystallized points toward the insurance completeness sum (HX).
            // A withdrawn stake would force want=0 below, but append rejects 0-amount entries so it
            // can never be a valid entry (it mismatches and rejects before completeness matters);
            // forfeited stakes are instead supplied as trailing extras after the entry loop.
            sealed_insurance_points = sealed_insurance_points.saturating_add(stake.points);
            let (principal, start, withdrawn) = read_subledger_position(&position.try_borrow_data()?)?;
            if withdrawn {
                0
            } else {
                // Cap by the LIVE position (finding HE): a depositor who PARTIALLY withdrew since
                // crystallize (principal reduced, but withdrawn=false until full exit) would otherwise
                // keep their stale-high crystallized `stake.points` and claim COIN for capital they no
                // longer have at risk, diluting honest insurance depositors. Use min(crystallized,
                // live-derived) so the amount never exceeds what the live position justifies. An
                // unchanged honest position has live_pts >= stake.points (more tenure) -> no change.
                let live_pts = insurance_points(seal_slot, principal, start, withdrawn);
                let pts = if stake.points < live_pts { stake.points } else { live_pts };
                points_to_amount(insurance_supply, pts, config.insurance_total_points)
            }
        };
        let off = DIST_PROPOSAL_HEADER + i * DIST_ENTRY_SIZE;
        let entry_recipient = pk(&pd, off);
        let entry_amount = u64::from_le_bytes(pd[off + 32..off + 40].try_into().unwrap());
        if entry_recipient != stake.recipient || entry_amount != want {
            return Err(ProgramError::InvalidAccountData); // not the deterministic distribution
        }
    }
    drop(pd);

    // Completeness for the residual cohort (finding HD): every residual stake's points must be
    // represented. seal is permissionless and one-shot — without this a cranker could front-run with
    // a proposal that OMITS residual LPs, sealing it irreversibly so the omitted LPs get 0 COIN (their
    // share burned) while the included parties' relative governance inflates. The residual cohort has
    // no forfeiture, so the sealed residual points must equal the full total. (The insurance cohort
    // gets the SAME guarantee below in finding HX — forfeited depositors are supplied as extras so the
    // completeness sum can still require every crystallized insurance stake to be represented.)
    // NOTE: the residual completeness sum is checked AFTER the extras loop below, because a residual DUST
    // stake (amount rounds to 0) is represented there as a zero-pay extra (finding IL), not as an entry.

    // Completeness for the insurance cohort (finding HX; the dual of HD above). seal is permissionless
    // and one-shot, so without this a malicious cranker could front-run with a proposal that pays every
    // OTHER party correctly but silently OMITS a non-forfeited insurance depositor, sealing it
    // irreversibly -> the omitted depositor gets 0 COIN forever (their crystallized share burned) with
    // no recourse. Forfeited (withdrawn) depositors legitimately get amount 0, and distribution::append
    // rejects 0-amount entries, so they cannot be proposal entries; the cranker passes them HERE as
    // trailing (stake, position) extras. Their points still count toward insurance_total_points (HR
    // keeps them so the forfeited share is burned, not paid) and thus toward the completeness sum. The
    // upshot: EVERY crystallized insurance stake must be represented (active entry OR forfeited extra),
    // exactly mirroring the residual cohort's HD guarantee.
    // Completeness EXTRAS (finding HX + IL): trailing stakes that are crystallized (their points are in
    // the totals, so completeness REQUIRES them) but whose deterministic seal amount is 0 — they cannot
    // be distribution entries (append rejects amount==0 / default), and were previously only acceptable
    // if INSURANCE+withdrawn (HX forfeiture). A stake's amount legitimately rounds to 0 in TWO cases:
    // (a) an insurance forfeiture (withdrawn -> 0), or (b) a DUST stake in EITHER cohort whose points
    // round to a 0 amount (points*supply < total — reachable on an HONEST genesis once total_points >
    // supply, e.g. a minnow beside a whale, which otherwise made the seal UN-completable = liveness DOS,
    // finding IL). Such a stake is supplied here, counted toward its cohort's completeness sum, and paid
    // NOTHING. The `amount == 0` check is the SAFETY: a stake owed a NONZERO amount is rejected here and
    // must be a paid entry, so no depositor is silently zeroed.
    loop {
        let stake_account = match next_account_info(iter) {
            Ok(a) => a,
            Err(_) => break, // no more accounts -> all extras consumed
        };
        if stake_account.owner != program_id {
            return Err(ProgramError::IllegalOwner);
        }
        if seen.contains(stake_account.key) {
            return Err(ProgramError::InvalidAccountData); // a stake cannot be counted twice
        }
        seen.push(*stake_account.key);
        let stake = Stake::deserialize(&stake_account.try_borrow_data()?)?;
        if stake.config != *config_account.key {
            return Err(ProgramError::InvalidAccountData);
        }
        let amount = if stake.cohort == COHORT_RESIDUAL {
            sealed_residual_points = sealed_residual_points.saturating_add(stake.points);
            points_to_amount(residual_supply, stake.points, config.total_points)
        } else {
            let position = next_account_info(iter)?;
            if *position.key != stake.backing_ledger || *position.owner != config.subledger_program {
                return Err(ProgramError::InvalidAccountData);
            }
            sealed_insurance_points = sealed_insurance_points.saturating_add(stake.points);
            let (principal, start, withdrawn) = read_subledger_position(&position.try_borrow_data()?)?;
            if withdrawn {
                0
            } else {
                let live_pts = insurance_points(seal_slot, principal, start, withdrawn);
                let pts = if stake.points < live_pts { stake.points } else { live_pts };
                points_to_amount(insurance_supply, pts, config.insurance_total_points)
            }
        };
        // An extra MUST be a zero-pay stake (forfeiture or dust). A stake owed a nonzero amount is
        // rejected -> it has to be a paid proposal entry, so this can never silently zero a depositor.
        if amount != 0 {
            return Err(ProgramError::InvalidAccountData);
        }
    }
    // Both cohorts complete: every crystallized stake is represented as a paid entry OR a zero-pay extra.
    if sealed_residual_points != config.total_points
        || sealed_insurance_points != config.insurance_total_points
    {
        return Err(ProgramError::InvalidAccountData);
    }

    let bump_arr = [config.bump];
    let signer_seeds: [&[u8]; 3] = [b"rd_config", config.coin_mint.as_ref(), &bump_arr];
    let ix = Instruction {
        program_id: *distribution_program.key,
        accounts: vec![
            AccountMeta::new_readonly(*config_account.key, true),
            AccountMeta::new(*distribution_config.key, false),
            AccountMeta::new(*proposal.key, false),
        ],
        data: vec![DIST_IX_SEAL_WINNER],
    };
    invoke_signed(
        &ix,
        &[
            config_account.clone(),
            distribution_config.clone(),
            proposal.clone(),
            distribution_program.clone(),
        ],
        &[&signer_seeds],
    )?;

    config.sealed = 1;
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
    // rd_config is SOLELY rd-controlled. Verified by `set_authority_clears_delegate_no_vault_rug`.
    if v.owner != *config_account.key || v.mint != config.coin_mint || v.amount < config.total_supply {
        return Err(ProgramError::InvalidAccountData);
    }
    config.vault = *vault.key;
    config.frozen_total_points = config.total_points;
    config.frozen_insurance_total_points = config.insurance_total_points;
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
    // The COIN must land in the bound recipient's own account (finding GY: no cranker redirect).
    let ra = spl_token::state::Account::unpack(&recipient_ata.try_borrow_data()?)?;
    if ra.owner != stake.recipient || ra.mint != config.coin_mint {
        return Err(ProgramError::InvalidAccountData);
    }
    let insurance_supply =
        ((config.total_supply as u128) * config.insurance_bps as u128 / BPS_DENOMINATOR as u128) as u64;
    let residual_supply = config.total_supply - insurance_supply;
    let amount = if stake.cohort == COHORT_RESIDUAL {
        // Residual points are frozen-cumulative (loss only grows) -> no live dependency, no HE.
        points_to_amount(residual_supply, stake.points, config.frozen_total_points)
    } else {
        // Insurance: read the LIVE subledger position NOW and cap by it ATOMICALLY (finding HE/JC).
        // A depositor who withdrew capital after freeze has a reduced live principal -> live_pts drops
        // -> they claim less; a full exit -> live_pts 0 -> 0 COIN (forfeit, HR). Tenure is measured to
        // freeze_slot (the consistent split point, the seal_slot analog) so all depositors compare
        // equally; only the live principal/withdrawn (read at claim) varies. read+cap+pay in ONE tx,
        // so there is no finalize/claim gap to over-claim through.
        let position = next_account_info(iter)?;
        if *position.key != stake.backing_ledger || *position.owner != config.subledger_program {
            return Err(ProgramError::InvalidAccountData);
        }
        let (principal, start, withdrawn) = read_subledger_position(&position.try_borrow_data()?)?;
        let live_pts = insurance_points(config.freeze_slot, principal, start, withdrawn);
        let pts = if stake.points < live_pts { stake.points } else { live_pts };
        points_to_amount(insurance_supply, pts, config.frozen_insurance_total_points)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_time_is_weak() {
        assert_eq!(floor_log2(0), 0);
        assert_eq!(floor_log2(1), 0);
        assert_eq!(floor_log2(1024), 10);
        assert_eq!(floor_log2(45 * 24 * 3600 * 2), 22);
        assert_eq!(floor_log2(7 * 24 * 3600 * 2), 20);
    }
    #[test]
    fn fee_cap_bounds_eligible() {
        assert_eq!(fee_supported_eligible(100, 10, 80), 100);
        assert_eq!(fee_supported_eligible(10_000, 10, 80), 1250);
        assert_eq!(fee_supported_eligible(10_000, 0, 80), 0);
    }
    #[test]
    fn jit_window_earns_almost_nothing() {
        assert_eq!(window_points(1_000, 1_000_000, 80, 1), 0);
        assert!(window_points(1_000, 1_000_000, 80, 1_000_000) > 0);
    }
    #[test]
    fn distribution_is_pro_rata_and_never_over_allocates() {
        assert_eq!(points_to_amount(1_000_000, 30, 100), 300_000);
        assert_eq!(points_to_amount(1_000_000, 70, 100), 700_000);
        assert!(points_to_amount(1_000_000, 30, 100) + points_to_amount(1_000_000, 70, 100) <= 1_000_000);
        assert_eq!(points_to_amount(1_000_000, 1, 0), 0);
    }

    #[test]
    fn insurance_exit_forfeits_its_coin_share() {
        // Live position (principal 100, joined slot 100, seal slot 1100): 100*floor_log2(1000)=900.
        assert_eq!(insurance_points(1100, 100, 100, false), 900);
        // After EXIT (subledger zeroes principal + sets withdrawn): 0 points -> COIN forfeited.
        assert_eq!(insurance_points(1100, 0, 100, true), 0);
        // Defensive: withdrawn flag alone forfeits even if principal not yet zeroed.
        assert_eq!(insurance_points(1100, 100, 100, true), 0);
    }

    #[test]
    fn reads_live_subledger_position_offsets() {
        let mut d = [0u8; 120];
        d[72..80].copy_from_slice(&100u64.to_le_bytes());
        d[88] = 1; // withdrawn
        d[89..97].copy_from_slice(&4242u64.to_le_bytes());
        let (p, s, w) = read_subledger_position(&d).unwrap();
        assert_eq!(p, 100);
        assert_eq!(s, 4242);
        assert!(w);
    }
}
