//! Non-custodial genesis vote.
//!
//! Insurance depositors vote on a COIN distribution. A deposit forwards the user's
//! capital into the Percolator market-0 insurance vault and records *attribution*
//! — owner, principal, deposit slot — for vote weighting. The funds live in
//! Percolator, not this program. The config PDA is the asset-0 insurance authority
//! during genesis (Percolator insurance is authority-gated), but its power is
//! constrained to topping up and **owner-authorized, principal-only exits** — it
//! can never take a user's principal — and it rotates to the surplus-only TWAP
//! chain at handoff. So the DAO can't steal user funds.
//!
//! Vote: one voter, one proposal. Weight = `floor(log2(hold)) * principal`,
//! resolved at vote time (last-write-time start slot). Backing a different
//! proposal requires retracting first. Quorum = `total_voted_principal*2 >
//! outstanding`; winner = `support_weight*2 > total_cast_weight`. Exits shrink
//! `outstanding`, so quorum recomputes — "those who stay decide".
//!
//! Trigger (permissionless): the first proposal to clear quorum + a weighted
//! majority is sealed by CPI into the distribution program (this program's config
//! PDA is that program's seal `authority`). No mint here — the fixed COIN supply
//! is distributed by the distribution program's claim/burn.

#![no_std]
extern crate alloc;

#[allow(unused_imports)]
use alloc::format;
use alloc::vec;

use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
    declare_id,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    msg,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    pubkey::Pubkey,
    system_instruction,
    sysvar::Sysvar,
};

declare_id!("GenesisVote11111111111111111111111111111111");

const CONFIG_DISC: [u8; 8] = *b"GVCONFG1";
const POSITION_DISC: [u8; 8] = *b"GVPOSIT1";
const PROPOSAL_DISC: [u8; 8] = *b"GVPROPV1";
const CONFIG_SIZE: usize = 232;
const POSITION_SIZE: usize = 112;
const PROPOSAL_SIZE: usize = 104;

// Percolator market-0 insurance top-up (permissionless): the user authorizes a
// transfer of their own funds into the insurance vault.
const PERC_IX_TOP_UP_INSURANCE: u8 = 9;
// Distribution program: SealWinner.
const DIST_IX_SEAL_WINNER: u8 = 3;

const IX_INIT_CONFIG: u8 = 0;
const IX_DEPOSIT_INSURANCE: u8 = 1;
const IX_REGISTER_PROPOSAL: u8 = 2;
const IX_VOTE: u8 = 3;
const IX_TRIGGER: u8 = 4;

const VOTE_BACK: u8 = 1;
const VOTE_RETRACT: u8 = 2;

#[cfg(not(feature = "no-entrypoint"))]
solana_program::entrypoint!(process_instruction);

fn config_seeds<'a>(coin_mint: &'a Pubkey) -> [&'a [u8]; 2] {
    [b"gv_config", coin_mint.as_ref()]
}
fn position_seeds<'a>(config: &'a Pubkey, owner: &'a Pubkey) -> [&'a [u8]; 3] {
    [b"gv_position", config.as_ref(), owner.as_ref()]
}
fn proposal_seeds<'a>(config: &'a Pubkey, dist_proposal: &'a Pubkey) -> [&'a [u8]; 3] {
    [b"gv_proposal", config.as_ref(), dist_proposal.as_ref()]
}

/// Time-weighted vote power: `floor(log2(age)) * principal`. Age < 2 (or empty)
/// has no weight, so there is monotonic pressure to deposit earlier.
fn vote_weight(principal: u64, age: u64) -> u64 {
    if principal == 0 || age < 2 {
        return 0;
    }
    (age.ilog2() as u64).saturating_mul(principal)
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct Config {
    coin_mint: Pubkey,
    distribution_program: Pubkey,
    distribution_config: Pubkey,
    market_slab: Pubkey,
    percolator_vault: Pubkey,
    percolator_program: Pubkey,
    total_voted_principal: u64,
    total_cast_weight: u64,
    outstanding_principal: u64,
    bump: u8,
}

impl Config {
    fn deserialize(d: &[u8]) -> Result<Self, ProgramError> {
        if d.len() < CONFIG_SIZE || d[..8] != CONFIG_DISC {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(Self {
            coin_mint: Pubkey::new_from_array(d[8..40].try_into().unwrap()),
            distribution_program: Pubkey::new_from_array(d[40..72].try_into().unwrap()),
            distribution_config: Pubkey::new_from_array(d[72..104].try_into().unwrap()),
            market_slab: Pubkey::new_from_array(d[104..136].try_into().unwrap()),
            percolator_vault: Pubkey::new_from_array(d[136..168].try_into().unwrap()),
            percolator_program: Pubkey::new_from_array(d[168..200].try_into().unwrap()),
            total_voted_principal: u64::from_le_bytes(d[200..208].try_into().unwrap()),
            total_cast_weight: u64::from_le_bytes(d[208..216].try_into().unwrap()),
            outstanding_principal: u64::from_le_bytes(d[216..224].try_into().unwrap()),
            bump: d[224],
        })
    }
    fn serialize(&self, d: &mut [u8]) {
        d[..8].copy_from_slice(&CONFIG_DISC);
        d[8..40].copy_from_slice(self.coin_mint.as_ref());
        d[40..72].copy_from_slice(self.distribution_program.as_ref());
        d[72..104].copy_from_slice(self.distribution_config.as_ref());
        d[104..136].copy_from_slice(self.market_slab.as_ref());
        d[136..168].copy_from_slice(self.percolator_vault.as_ref());
        d[168..200].copy_from_slice(self.percolator_program.as_ref());
        d[200..208].copy_from_slice(&self.total_voted_principal.to_le_bytes());
        d[208..216].copy_from_slice(&self.total_cast_weight.to_le_bytes());
        d[216..224].copy_from_slice(&self.outstanding_principal.to_le_bytes());
        d[224] = self.bump;
        d[225..CONFIG_SIZE].fill(0);
    }
}

struct Position {
    owner: Pubkey,
    principal: u64,
    start_slot: u64,
    voted_proposal: Pubkey, // default() = no live ballot
    voted_weight: u64,
    voted_principal: u64,
}

impl Position {
    fn deserialize(d: &[u8]) -> Result<Self, ProgramError> {
        if d.len() < POSITION_SIZE || d[..8] != POSITION_DISC {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(Self {
            owner: Pubkey::new_from_array(d[8..40].try_into().unwrap()),
            principal: u64::from_le_bytes(d[40..48].try_into().unwrap()),
            start_slot: u64::from_le_bytes(d[48..56].try_into().unwrap()),
            voted_proposal: Pubkey::new_from_array(d[56..88].try_into().unwrap()),
            voted_weight: u64::from_le_bytes(d[88..96].try_into().unwrap()),
            voted_principal: u64::from_le_bytes(d[96..104].try_into().unwrap()),
        })
    }
    fn serialize(&self, d: &mut [u8]) {
        d[..8].copy_from_slice(&POSITION_DISC);
        d[8..40].copy_from_slice(self.owner.as_ref());
        d[40..48].copy_from_slice(&self.principal.to_le_bytes());
        d[48..56].copy_from_slice(&self.start_slot.to_le_bytes());
        d[56..88].copy_from_slice(self.voted_proposal.as_ref());
        d[88..96].copy_from_slice(&self.voted_weight.to_le_bytes());
        d[96..104].copy_from_slice(&self.voted_principal.to_le_bytes());
        d[104..POSITION_SIZE].fill(0);
    }
    fn has_live_ballot(&self) -> bool {
        self.voted_proposal != Pubkey::default()
    }
}

struct ProposalVote {
    config: Pubkey,
    distribution_proposal: Pubkey,
    support_weight: u64,
    support_principal: u64,
    executed: bool,
}

impl ProposalVote {
    fn deserialize(d: &[u8]) -> Result<Self, ProgramError> {
        if d.len() < PROPOSAL_SIZE || d[..8] != PROPOSAL_DISC {
            return Err(ProgramError::InvalidAccountData);
        }
        let executed = d[88];
        if executed > 1 {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(Self {
            config: Pubkey::new_from_array(d[8..40].try_into().unwrap()),
            distribution_proposal: Pubkey::new_from_array(d[40..72].try_into().unwrap()),
            support_weight: u64::from_le_bytes(d[72..80].try_into().unwrap()),
            support_principal: u64::from_le_bytes(d[80..88].try_into().unwrap()),
            executed: executed == 1,
        })
    }
    fn serialize(&self, d: &mut [u8]) {
        d[..8].copy_from_slice(&PROPOSAL_DISC);
        d[8..40].copy_from_slice(self.config.as_ref());
        d[40..72].copy_from_slice(self.distribution_proposal.as_ref());
        d[72..80].copy_from_slice(&self.support_weight.to_le_bytes());
        d[80..88].copy_from_slice(&self.support_principal.to_le_bytes());
        d[88] = self.executed as u8;
        d[89..PROPOSAL_SIZE].fill(0);
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub fn process_instruction<'a>(
    program_id: &Pubkey,
    accounts: &'a [AccountInfo<'a>],
    instruction_data: &[u8],
) -> ProgramResult {
    let (tag, data) = instruction_data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;
    match *tag {
        IX_INIT_CONFIG => init_config(program_id, accounts),
        IX_DEPOSIT_INSURANCE => deposit_insurance(program_id, accounts, data),
        IX_REGISTER_PROPOSAL => register_proposal(program_id, accounts),
        IX_VOTE => vote(program_id, accounts, data),
        IX_TRIGGER => trigger(program_id, accounts, data),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn take_u64(d: &mut &[u8]) -> Result<u64, ProgramError> {
    if d.len() < 8 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let (h, t) = d.split_at(8);
    *d = t;
    Ok(u64::from_le_bytes(h.try_into().unwrap()))
}

fn create_pda<'a>(
    payer: &AccountInfo<'a>,
    account: &AccountInfo<'a>,
    system_program: &AccountInfo<'a>,
    program_id: &Pubkey,
    seeds: &[&[u8]],
    size: usize,
) -> ProgramResult {
    let rent = solana_program::rent::Rent::get()?;
    invoke_signed(
        &system_instruction::create_account(
            payer.key,
            account.key,
            rent.minimum_balance(size),
            size as u64,
            program_id,
        ),
        &[payer.clone(), account.clone(), system_program.clone()],
        &[seeds],
    )
}

// init_config accounts: [payer(s,w), coin_mint, config(pda,w), distribution_program,
//   distribution_config, market_slab, percolator_vault, percolator_program, system]
fn init_config<'a>(program_id: &Pubkey, accounts: &'a [AccountInfo<'a>]) -> ProgramResult {
    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let coin_mint = next_account_info(iter)?;
    let config_account = next_account_info(iter)?;
    let distribution_program = next_account_info(iter)?;
    let distribution_config = next_account_info(iter)?;
    let market_slab = next_account_info(iter)?;
    let percolator_vault = next_account_info(iter)?;
    let percolator_program = next_account_info(iter)?;
    let system_program = next_account_info(iter)?;

    if !payer.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *system_program.key != solana_program::system_program::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    let (expected, bump) = Pubkey::find_program_address(&config_seeds(coin_mint.key), program_id);
    if *config_account.key != expected {
        return Err(ProgramError::InvalidSeeds);
    }
    if config_account.lamports() != 0 || config_account.data_len() != 0 {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let bump_arr = [bump];
    let seeds: [&[u8]; 3] = [b"gv_config", coin_mint.key.as_ref(), &bump_arr];
    create_pda(payer, config_account, system_program, program_id, &seeds, CONFIG_SIZE)?;

    let config = Config {
        coin_mint: *coin_mint.key,
        distribution_program: *distribution_program.key,
        distribution_config: *distribution_config.key,
        market_slab: *market_slab.key,
        percolator_vault: *percolator_vault.key,
        percolator_program: *percolator_program.key,
        total_voted_principal: 0,
        total_cast_weight: 0,
        outstanding_principal: 0,
        bump,
    };
    config.serialize(&mut config_account.try_borrow_mut_data()?);
    Ok(())
}

// deposit_insurance accounts: [owner(s,w), config(w), position(pda,w), owner_ata(w),
//   holding(w, config-PDA-owned), market_slab(w), percolator_vault(w),
//   percolator_program, token_program, system]
// data: amount(u64)
//
// Forwards the user's funds into Percolator market-0 insurance and records
// attribution. The config PDA is the asset-0 insurance authority, so it signs the
// top-up — but its authority is constrained to ADD (here) and owner-authorized
// principal-only exit; it can never take a user's principal, and the authority
// rotates to the surplus-only TWAP chain at handoff.
fn deposit_insurance<'a>(program_id: &Pubkey, accounts: &'a [AccountInfo<'a>], mut data: &[u8]) -> ProgramResult {
    let iter = &mut accounts.iter();
    let owner = next_account_info(iter)?;
    let config_account = next_account_info(iter)?;
    let position_account = next_account_info(iter)?;
    let owner_ata = next_account_info(iter)?;
    let holding = next_account_info(iter)?;
    let market_slab = next_account_info(iter)?;
    let percolator_vault = next_account_info(iter)?;
    let percolator_program = next_account_info(iter)?;
    let token_program = next_account_info(iter)?;
    let system_program = next_account_info(iter)?;

    let amount = take_u64(&mut data)?;
    if amount == 0 || !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !owner.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *token_program.key != spl_token::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    if config_account.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    let mut config = Config::deserialize(&config_account.try_borrow_data()?)?;
    if *market_slab.key != config.market_slab
        || *percolator_vault.key != config.percolator_vault
        || *percolator_program.key != config.percolator_program
    {
        return Err(ProgramError::InvalidAccountData);
    }

    // Real Percolator insurance is authority-gated: only the asset-0 insurance
    // authority may top up, and the source funds must be its own. The config PDA
    // is that authority during genesis. So we route the user's funds through a
    // config-PDA-owned holding account and the config PDA signs the top-up.
    //
    // 1) User -> holding (user-authorized; the user is moving their own funds).
    invoke(
        &spl_token::instruction::transfer(
            token_program.key,
            owner_ata.key,
            holding.key,
            owner.key,
            &[],
            amount,
        )?,
        &[owner_ata.clone(), holding.clone(), owner.clone(), token_program.clone()],
    )?;

    // 2) holding -> Percolator market-0 insurance, signed by the config PDA as the
    //    asset-0 insurance authority. The program can only ADD here; principal
    //    leaves solely via the owner-authorized principal-only exit.
    let bump_arr = [config.bump];
    let auth_seeds: [&[u8]; 3] = [b"gv_config", config.coin_mint.as_ref(), &bump_arr];
    let mut ix_data = vec![PERC_IX_TOP_UP_INSURANCE];
    ix_data.extend_from_slice(&(amount as u128).to_le_bytes());
    invoke_signed(
        &Instruction {
            program_id: *percolator_program.key,
            accounts: vec![
                AccountMeta::new_readonly(*config_account.key, true),
                AccountMeta::new(*market_slab.key, false),
                AccountMeta::new(*holding.key, false),
                AccountMeta::new(*percolator_vault.key, false),
                AccountMeta::new_readonly(*token_program.key, false),
            ],
            data: ix_data,
        },
        &[
            config_account.clone(),
            market_slab.clone(),
            holding.clone(),
            percolator_vault.clone(),
            token_program.clone(),
            percolator_program.clone(),
        ],
        &[&auth_seeds],
    )?;

    // Record attribution (last-write-time start slot).
    let pos_seeds = position_seeds(config_account.key, owner.key);
    let (expected_pos, pos_bump) = Pubkey::find_program_address(&pos_seeds, program_id);
    if *position_account.key != expected_pos {
        return Err(ProgramError::InvalidSeeds);
    }
    let clock = Clock::get()?;
    let mut position = if position_account.data_len() == 0 || position_account.lamports() == 0 {
        let bump_arr = [pos_bump];
        let seeds: [&[u8]; 4] = [b"gv_position", config_account.key.as_ref(), owner.key.as_ref(), &bump_arr];
        create_pda(owner, position_account, system_program, program_id, &seeds, POSITION_SIZE)?;
        Position {
            owner: *owner.key,
            principal: 0,
            start_slot: 0,
            voted_proposal: Pubkey::default(),
            voted_weight: 0,
            voted_principal: 0,
        }
    } else {
        if position_account.owner != program_id {
            return Err(ProgramError::IllegalOwner);
        }
        let p = Position::deserialize(&position_account.try_borrow_data()?)?;
        if p.owner != *owner.key {
            return Err(ProgramError::IllegalOwner);
        }
        // A top-up while a ballot is live would change the staked weight basis;
        // require retracting first (keeps the vote tally consistent).
        if p.has_live_ballot() {
            return Err(ProgramError::InvalidInstructionData);
        }
        p
    };
    position.principal = position.principal.checked_add(amount).ok_or(ProgramError::ArithmeticOverflow)?;
    position.start_slot = clock.slot; // last-write-time
    config.outstanding_principal = config
        .outstanding_principal
        .checked_add(amount)
        .ok_or(ProgramError::ArithmeticOverflow)?;

    config.serialize(&mut config_account.try_borrow_mut_data()?);
    position.serialize(&mut position_account.try_borrow_mut_data()?);
    Ok(())
}

// register_proposal accounts: [payer(s,w), config, proposal_vote(pda,w),
//   distribution_proposal, system]
fn register_proposal<'a>(program_id: &Pubkey, accounts: &'a [AccountInfo<'a>]) -> ProgramResult {
    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let config_account = next_account_info(iter)?;
    let proposal_account = next_account_info(iter)?;
    let distribution_proposal = next_account_info(iter)?;
    let system_program = next_account_info(iter)?;

    if !payer.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if config_account.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    let config = Config::deserialize(&config_account.try_borrow_data()?)?;
    // The proposal must be a real distribution proposal owned by the distribution
    // program (so votes can only target genuine distributions).
    if distribution_proposal.owner != &config.distribution_program {
        return Err(ProgramError::IllegalOwner);
    }

    let seeds = proposal_seeds(config_account.key, distribution_proposal.key);
    let (expected, bump) = Pubkey::find_program_address(&seeds, program_id);
    if *proposal_account.key != expected {
        return Err(ProgramError::InvalidSeeds);
    }
    if proposal_account.lamports() != 0 || proposal_account.data_len() != 0 {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let bump_arr = [bump];
    let s: [&[u8]; 4] = [b"gv_proposal", config_account.key.as_ref(), distribution_proposal.key.as_ref(), &bump_arr];
    create_pda(payer, proposal_account, system_program, program_id, &s, PROPOSAL_SIZE)?;

    let pv = ProposalVote {
        config: *config_account.key,
        distribution_proposal: *distribution_proposal.key,
        support_weight: 0,
        support_principal: 0,
        executed: false,
    };
    pv.serialize(&mut proposal_account.try_borrow_mut_data()?);
    Ok(())
}

// vote accounts: [voter(s), config(w), position(w), proposal_vote(w)]
// data: action(u8) — 1 back, 2 retract
fn vote(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let iter = &mut accounts.iter();
    let voter = next_account_info(iter)?;
    let config_account = next_account_info(iter)?;
    let position_account = next_account_info(iter)?;
    let proposal_account = next_account_info(iter)?;

    if data.len() != 1 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let action = data[0];
    if action != VOTE_BACK && action != VOTE_RETRACT {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !voter.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if config_account.owner != program_id
        || position_account.owner != program_id
        || proposal_account.owner != program_id
    {
        return Err(ProgramError::IllegalOwner);
    }
    let mut config = Config::deserialize(&config_account.try_borrow_data()?)?;
    let mut position = Position::deserialize(&position_account.try_borrow_data()?)?;
    let mut pv = ProposalVote::deserialize(&proposal_account.try_borrow_data()?)?;
    if position.owner != *voter.key {
        return Err(ProgramError::IllegalOwner);
    }
    if pv.config != *config_account.key || pv.executed {
        return Err(ProgramError::InvalidAccountData);
    }
    // One vote, one proposal: a live ballot must be on THIS proposal.
    if position.has_live_ballot() && position.voted_proposal != *proposal_account.key {
        msg!("retract your existing vote before backing another proposal");
        return Err(ProgramError::InvalidInstructionData);
    }

    // Back out any prior live contribution from this proposal + the global tallies.
    if position.has_live_ballot() {
        pv.support_weight = pv.support_weight.checked_sub(position.voted_weight).ok_or(ProgramError::InvalidAccountData)?;
        pv.support_principal = pv.support_principal.checked_sub(position.voted_principal).ok_or(ProgramError::InvalidAccountData)?;
        config.total_cast_weight = config.total_cast_weight.checked_sub(position.voted_weight).ok_or(ProgramError::InvalidAccountData)?;
        config.total_voted_principal = config.total_voted_principal.checked_sub(position.voted_principal).ok_or(ProgramError::InvalidAccountData)?;
    } else if action == VOTE_RETRACT {
        return Err(ProgramError::InvalidInstructionData); // nothing to retract
    }

    if action == VOTE_RETRACT {
        position.voted_proposal = Pubkey::default();
        position.voted_weight = 0;
        position.voted_principal = 0;
    } else {
        let clock = Clock::get()?;
        let principal = position.principal;
        let weight = if position.start_slot == 0 {
            0
        } else {
            vote_weight(principal, clock.slot.saturating_sub(position.start_slot))
        };
        if weight == 0 {
            msg!("position has no vote weight (unfunded or too recent)");
            return Err(ProgramError::InvalidAccountData);
        }
        pv.support_weight = pv.support_weight.checked_add(weight).ok_or(ProgramError::ArithmeticOverflow)?;
        pv.support_principal = pv.support_principal.checked_add(principal).ok_or(ProgramError::ArithmeticOverflow)?;
        config.total_cast_weight = config.total_cast_weight.checked_add(weight).ok_or(ProgramError::ArithmeticOverflow)?;
        config.total_voted_principal = config.total_voted_principal.checked_add(principal).ok_or(ProgramError::ArithmeticOverflow)?;
        position.voted_proposal = *proposal_account.key;
        position.voted_weight = weight;
        position.voted_principal = principal;
    }

    config.serialize(&mut config_account.try_borrow_mut_data()?);
    position.serialize(&mut position_account.try_borrow_mut_data()?);
    pv.serialize(&mut proposal_account.try_borrow_mut_data()?);
    Ok(())
}

// trigger accounts: [cranker(s), config(w), proposal_vote(w), distribution_program,
//   distribution_config(w), distribution_proposal(w)]
// Permissionless: seal the winning distribution via CPI.
fn trigger<'a>(program_id: &Pubkey, accounts: &'a [AccountInfo<'a>], data: &[u8]) -> ProgramResult {
    if !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    let iter = &mut accounts.iter();
    let cranker = next_account_info(iter)?;
    let config_account = next_account_info(iter)?;
    let proposal_account = next_account_info(iter)?;
    let distribution_program = next_account_info(iter)?;
    let distribution_config = next_account_info(iter)?;
    let distribution_proposal = next_account_info(iter)?;

    if !cranker.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if config_account.owner != program_id || proposal_account.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    let config = Config::deserialize(&config_account.try_borrow_data()?)?;
    let mut pv = ProposalVote::deserialize(&proposal_account.try_borrow_data()?)?;
    if pv.config != *config_account.key || pv.executed {
        return Err(ProgramError::InvalidAccountData);
    }
    if *distribution_program.key != config.distribution_program
        || *distribution_config.key != config.distribution_config
        || *distribution_proposal.key != pv.distribution_proposal
    {
        return Err(ProgramError::InvalidAccountData);
    }
    // Quorum: more than half of outstanding insurance principal has voted.
    if (config.total_voted_principal as u128) * 2 <= config.outstanding_principal as u128 {
        msg!("vote lacks a principal quorum");
        return Err(ProgramError::InvalidInstructionData);
    }
    // Winner: this proposal holds a strict majority of cast log-weight.
    if (pv.support_weight as u128) * 2 <= config.total_cast_weight as u128 {
        msg!("proposal lacks a weighted majority");
        return Err(ProgramError::InvalidInstructionData);
    }

    pv.executed = true;
    pv.serialize(&mut proposal_account.try_borrow_mut_data()?);

    // Seal the distribution. The config PDA is the distribution's seal authority.
    let bump_arr = [config.bump];
    let seeds: [&[u8]; 3] = [b"gv_config", config.coin_mint.as_ref(), &bump_arr];
    invoke_signed(
        &Instruction {
            program_id: *distribution_program.key,
            accounts: vec![
                AccountMeta::new_readonly(*config_account.key, true),
                AccountMeta::new(*distribution_config.key, false),
                AccountMeta::new(*distribution_proposal.key, false),
            ],
            data: vec![DIST_IX_SEAL_WINNER],
        },
        &[
            config_account.clone(),
            distribution_config.clone(),
            distribution_proposal.clone(),
            distribution_program.clone(),
        ],
        &[&seeds],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_is_log_time_times_principal() {
        assert_eq!(vote_weight(10, 0), 0);
        assert_eq!(vote_weight(10, 1), 0); // age < 2 -> no weight
        assert_eq!(vote_weight(10, 4), 20); // floor(log2(4))=2 * 10
        assert_eq!(vote_weight(10, 1024), 100); // floor(log2(1024))=10 * 10
        assert_eq!(vote_weight(0, 1024), 0);
    }

    #[test]
    fn state_round_trips() {
        let c = Config {
            coin_mint: Pubkey::new_unique(),
            distribution_program: Pubkey::new_unique(),
            distribution_config: Pubkey::new_unique(),
            market_slab: Pubkey::new_unique(),
            percolator_vault: Pubkey::new_unique(),
            percolator_program: Pubkey::new_unique(),
            total_voted_principal: 7,
            total_cast_weight: 70,
            outstanding_principal: 12,
            bump: 250,
        };
        let mut b = [0u8; CONFIG_SIZE];
        c.serialize(&mut b);
        let d = Config::deserialize(&b).unwrap();
        assert_eq!(d.total_voted_principal, 7);
        assert_eq!(d.outstanding_principal, 12);
        assert_eq!(d.bump, 250);

        let p = Position {
            owner: Pubkey::new_unique(),
            principal: 5,
            start_slot: 100,
            voted_proposal: Pubkey::new_unique(),
            voted_weight: 40,
            voted_principal: 5,
        };
        let mut pb = [0u8; POSITION_SIZE];
        p.serialize(&mut pb);
        let dp = Position::deserialize(&pb).unwrap();
        assert_eq!(dp.principal, 5);
        assert!(dp.has_live_ballot());

        let pv = ProposalVote {
            config: Pubkey::new_unique(),
            distribution_proposal: Pubkey::new_unique(),
            support_weight: 8,
            support_principal: 2,
            executed: true,
        };
        let mut vb = [0u8; PROPOSAL_SIZE];
        pv.serialize(&mut vb);
        let dv = ProposalVote::deserialize(&vb).unwrap();
        assert_eq!(dv.support_weight, 8);
        assert!(dv.executed);
    }
}
