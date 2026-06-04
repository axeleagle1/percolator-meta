//! Asset-local insurance / backing subledger.
//!
//! A reusable, **owner-bound** deposit pool that permissionless asset programs
//! (Percolator markets/assets 1..N) can use to offer local insurance/backing
//! deposits that earn local fees/yield. It is deliberately *not* part of genesis
//! COIN farming and the MetaDAO has **no authority over it** — there is no admin,
//! no governance key, no upgrade-of-policy path. Each depositor can always exit
//! their own position; nobody else can move their funds.
//!
//! Accounting (per pool):
//!   - `outstanding_principal` = sum of un-withdrawn deposit principal.
//!   - `asset_balance`         = the pool vault's live token balance (principal +
//!     any fees/yield transferred in, minus impairment).
//!
//! Exit policy:
//!   - `Principal`    — pay `principal` when healthy (`balance >= outstanding`),
//!     pro-rata `balance * principal / outstanding` when impaired. Surplus stays
//!     in the pool.
//!   - `WithSurplus`  — always pro-rata `balance * principal / outstanding`, so
//!     local fees/yield are returned to depositors.

#![no_std]
extern crate alloc;

#[allow(unused_imports)]
use alloc::format; // required by the entrypoint!/msg! macro in SBF builds
use alloc::vec;
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
    declare_id,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    program_pack::Pack,
    pubkey::Pubkey,
    system_instruction,
    sysvar::Sysvar,
};

declare_id!("Sub1edger1111111111111111111111111111111111");

const POOL_DISC: [u8; 8] = *b"SUBPOOL1";
const POSITION_DISC: [u8; 8] = *b"SUBPOS01";
// Pool now also carries the Percolator refs (market_slab + percolator_program) so
// an insurance pool can sign TopUpInsurance / WithdrawInsuranceLimited as the
// asset-0 insurance authority/operator. Own-vault pools leave them zero. The trailing
// vote_authority (the genesis-vote config PDA) may toggle a position's vote-lock.
const POOL_SIZE: usize = 192;
const POSITION_SIZE: usize = 104;

const POLICY_PRINCIPAL: u8 = 0;
const POLICY_WITH_SURPLUS: u8 = 1;

// Which Percolator domain this pool backs. asset-0 insurance is the principal-only
// vote bond; backing (asset 0) and assets 1..N run with-surplus.
const DOMAIN_INSURANCE: u8 = 0;
const DOMAIN_BACKING: u8 = 1;

// The SPL Associated Token Account program. Percolator pins each market vault to
// the single CANONICAL ATA of (vault_authority, mint) — its finding F-VAULT-FRAG.
// We mirror that derivation so a pool can only ever bind to the exact vault
// Percolator will accept, failing fast at init instead of dead on first deposit.
const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey =
    solana_program::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

fn canonical_vault_address(vault_authority: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            vault_authority.as_ref(),
            spl_token::ID.as_ref(),
            mint.as_ref(),
        ],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    )
    .0
}

const IX_INIT_POOL: u8 = 0;
const IX_DEPOSIT: u8 = 1;
const IX_WITHDRAW: u8 = 2;
const IX_INIT_INSURANCE_POOL: u8 = 3;
const IX_INSURANCE_DEPOSIT: u8 = 4;
const IX_INSURANCE_WITHDRAW: u8 = 5;
// Toggle a position's vote-lock. Callable ONLY by the pool's registered
// vote_authority (the genesis-vote config PDA). While locked, the owner cannot
// insurance-withdraw — they must retract their genesis vote first, which clears
// the lock. This binds the vote's principal snapshot to capital that is still at
// risk (closes the vote-outlives-capital vector).
const IX_SET_VOTE_LOCK: u8 = 6;
// Consent to RECEIVE the asset-0 insurance authority + operator roles from the market's
// asset_admin (the Squads vault). The subledger never rotates keys itself — the Squads
// vault (driven by the DAO) is the asset_admin and the only thing that calls percolator
// UpdateAssetAuthority; this instruction only provides the pool's incoming co-signature
// so the grant can land. Mirror of the twap's accept_operator.
const IX_ACCEPT_OPERATOR: u8 = 7;

// Percolator CPI tags (verified against the real v16 program).
const PERC_IX_TOP_UP_INSURANCE: u8 = 9;
const PERC_IX_WITHDRAW_INSURANCE_LIMITED: u8 = 23;
const PERC_IX_UPDATE_ASSET_AUTHORITY: u8 = 65;
const ASSET_AUTH_INSURANCE: u8 = 1; // insurance_authority (gates TopUpInsurance)
const ASSET_AUTH_INSURANCE_OPERATOR: u8 = 2; // insurance_operator (gates WithdrawInsuranceLimited)

#[cfg(not(feature = "no-entrypoint"))]
solana_program::entrypoint!(process_instruction);

// The pool PDA commits to its market binding, not just (mint, asset_id). Keying it on
// (mint, asset_id) alone made init_insurance_pool (permissionless) front-run squattable:
// the genesis pool PDA = f(COIN_mint, 0) and the gv config PDA = f(COIN_mint) are both
// predictable, so an attacker could init the pool FIRST bound to a percolator market
// THEY control (passing that market's canonical insurance vault) with vote_authority set
// to the predictable real gv config PDA — satisfying the gv binding check. Genesis would
// then wire to a pool that routes every depositor's principal into the attacker's market
// (TopUpInsurance), where the attacker (its marketauth) can strand or bleed it: LOF, not
// just DOS. Folding market_slab + percolator_program into the seed means the only pool
// that can exist at the legit address is bound to the legit market (own-vault pools use
// Pubkey::default() for both, matching what they store). A squat with any other market
// lands at a different PDA the genesis ignores. (finding Q; same class as finding P.)
fn pool_seeds<'a>(
    mint: &'a Pubkey,
    asset_id: &'a [u8; 8],
    market_slab: &'a Pubkey,
    percolator_program: &'a Pubkey,
) -> [&'a [u8]; 5] {
    [
        b"subledger_pool",
        mint.as_ref(),
        asset_id,
        market_slab.as_ref(),
        percolator_program.as_ref(),
    ]
}

fn position_seeds<'a>(pool: &'a Pubkey, owner: &'a Pubkey) -> [&'a [u8]; 3] {
    [b"subledger_position", pool.as_ref(), owner.as_ref()]
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct Pool {
    mint: Pubkey,
    /// Percolator asset index this pool attributes (0 = market-0).
    asset_id: u64,
    /// The token account principal flows through. For own-vault pools this is the
    /// pool-PDA-owned SPL account; for insurance pools it is the Percolator
    /// market's canonical insurance vault (the ATA of its vault_authority).
    vault: Pubkey,
    /// `outstanding_principal` is the quorum denominator the genesis-vote reads:
    /// the sum of live (un-withdrawn) deposit principal in this pool.
    outstanding_principal: u64,
    policy: u8,
    domain: u8, // DOMAIN_INSURANCE | DOMAIN_BACKING
    bump: u8,
    /// Percolator market slab this insurance pool tops up / withdraws from.
    /// `Pubkey::default()` for own-vault pools.
    market_slab: Pubkey,
    /// Percolator program id. `Pubkey::default()` for own-vault pools.
    percolator_program: Pubkey,
    /// Authority allowed to toggle a position's vote-lock (the genesis-vote config
    /// PDA). `Pubkey::default()` disables vote-locking (own-vault pools).
    vote_authority: Pubkey,
}

impl Pool {
    fn deserialize(data: &[u8]) -> Result<Self, ProgramError> {
        if data.len() < POOL_SIZE || data[..8] != POOL_DISC {
            return Err(ProgramError::InvalidAccountData);
        }
        let policy = data[88];
        let domain = data[90];
        if policy > POLICY_WITH_SURPLUS || domain > DOMAIN_BACKING {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(Self {
            mint: Pubkey::new_from_array(data[8..40].try_into().unwrap()),
            asset_id: u64::from_le_bytes(data[40..48].try_into().unwrap()),
            vault: Pubkey::new_from_array(data[48..80].try_into().unwrap()),
            outstanding_principal: u64::from_le_bytes(data[80..88].try_into().unwrap()),
            policy,
            domain,
            bump: data[89],
            market_slab: Pubkey::new_from_array(data[96..128].try_into().unwrap()),
            percolator_program: Pubkey::new_from_array(data[128..160].try_into().unwrap()),
            vote_authority: Pubkey::new_from_array(data[160..192].try_into().unwrap()),
        })
    }

    fn serialize(&self, data: &mut [u8]) {
        data[..8].copy_from_slice(&POOL_DISC);
        data[8..40].copy_from_slice(self.mint.as_ref());
        data[40..48].copy_from_slice(&self.asset_id.to_le_bytes());
        data[48..80].copy_from_slice(self.vault.as_ref());
        data[80..88].copy_from_slice(&self.outstanding_principal.to_le_bytes());
        data[88] = self.policy;
        data[89] = self.bump;
        data[90] = self.domain;
        data[91..96].fill(0);
        data[96..128].copy_from_slice(self.market_slab.as_ref());
        data[128..160].copy_from_slice(self.percolator_program.as_ref());
        data[160..192].copy_from_slice(self.vote_authority.as_ref());
    }

    fn is_insurance(&self) -> bool {
        self.percolator_program != Pubkey::default()
    }
}

struct Position {
    pool: Pubkey,
    owner: Pubkey,
    /// Live principal (current deposit, less any withdrawal). The genesis-vote
    /// reads this with `start_slot` to compute `floor(log2(now-start)) * principal`.
    principal: u64,
    withdrawn_amount: u64,
    withdrawn: bool,
    /// Last-write-time of this position (set on deposit). Topping up resets it, so
    /// late additions don't earn early-join vote weight.
    start_slot: u64,
    /// Set by the pool's vote_authority while a genesis vote is live on this
    /// position. Blocks insurance-withdraw until the vote is retracted.
    vote_locked: bool,
}

impl Position {
    fn deserialize(data: &[u8]) -> Result<Self, ProgramError> {
        if data.len() < POSITION_SIZE || data[..8] != POSITION_DISC {
            return Err(ProgramError::InvalidAccountData);
        }
        let withdrawn = data[88];
        let vote_locked = data[97];
        if withdrawn > 1 || vote_locked > 1 {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(Self {
            pool: Pubkey::new_from_array(data[8..40].try_into().unwrap()),
            owner: Pubkey::new_from_array(data[40..72].try_into().unwrap()),
            principal: u64::from_le_bytes(data[72..80].try_into().unwrap()),
            withdrawn_amount: u64::from_le_bytes(data[80..88].try_into().unwrap()),
            withdrawn: withdrawn == 1,
            start_slot: u64::from_le_bytes(data[89..97].try_into().unwrap()),
            vote_locked: vote_locked == 1,
        })
    }

    fn serialize(&self, data: &mut [u8]) {
        data[..8].copy_from_slice(&POSITION_DISC);
        data[8..40].copy_from_slice(self.pool.as_ref());
        data[40..72].copy_from_slice(self.owner.as_ref());
        data[72..80].copy_from_slice(&self.principal.to_le_bytes());
        data[80..88].copy_from_slice(&self.withdrawn_amount.to_le_bytes());
        data[88] = self.withdrawn as u8;
        data[89..97].copy_from_slice(&self.start_slot.to_le_bytes());
        data[97] = self.vote_locked as u8;
        data[98..POSITION_SIZE].fill(0);
    }
}

// ---------------------------------------------------------------------------
// Pure payout logic (the ported subledger arithmetic)
// ---------------------------------------------------------------------------

fn mul_div_floor(a: u64, b: u64, denom: u64) -> Option<u64> {
    if denom == 0 {
        return None;
    }
    Some((a as u128 * b as u128 / denom as u128) as u64)
}

/// Payout for a full position exit. `balance` is the pool's live token balance.
fn payout(policy: u8, balance: u64, outstanding: u64, principal: u64) -> Result<u64, ProgramError> {
    if outstanding == 0 || principal == 0 || principal > outstanding {
        return Err(ProgramError::InvalidAccountData);
    }
    let pro_rata = mul_div_floor(balance, principal, outstanding).ok_or(ProgramError::ArithmeticOverflow)?;
    match policy {
        POLICY_PRINCIPAL => {
            if balance >= outstanding {
                Ok(principal) // healthy: principal only, surplus stays in the pool
            } else {
                Ok(pro_rata) // impaired: pro-rata
            }
        }
        POLICY_WITH_SURPLUS => Ok(pro_rata), // always pro-rata: yield returned
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    let (tag, mut data) = instruction_data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;
    match *tag {
        IX_INIT_POOL => process_init_pool(program_id, accounts, &mut data),
        IX_DEPOSIT => process_deposit(program_id, accounts, &mut data),
        IX_WITHDRAW => process_withdraw(program_id, accounts, &mut data),
        IX_INIT_INSURANCE_POOL => process_init_insurance_pool(program_id, accounts, &mut data),
        IX_INSURANCE_DEPOSIT => process_insurance_deposit(program_id, accounts, &mut data),
        IX_INSURANCE_WITHDRAW => process_insurance_withdraw(program_id, accounts, &mut data),
        IX_SET_VOTE_LOCK => process_set_vote_lock(program_id, accounts, &mut data),
        IX_ACCEPT_OPERATOR => process_accept_operator(program_id, accounts, &mut data),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn read_u64(data: &mut &[u8]) -> Result<u64, ProgramError> {
    if data.len() < 8 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let (head, tail) = data.split_at(8);
    *data = tail;
    Ok(u64::from_le_bytes(head.try_into().unwrap()))
}

fn read_u8(data: &mut &[u8]) -> Result<u8, ProgramError> {
    if data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    let (head, tail) = data.split_at(1);
    *data = tail;
    Ok(head[0])
}

fn token_balance(account: &AccountInfo) -> Result<u64, ProgramError> {
    if account.owner != &spl_token::ID {
        return Err(ProgramError::IllegalOwner);
    }
    Ok(spl_token::state::Account::unpack(&account.try_borrow_data()?)?.amount)
}

// init_pool accounts: [payer(s,w), mint, pool(w,pda), vault(token acct, authority=pool pda),
//                      system_program]
// data: asset_id (u64), policy (u8)
fn process_init_pool(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &mut &[u8],
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let mint = next_account_info(iter)?;
    let pool_account = next_account_info(iter)?;
    let vault = next_account_info(iter)?;
    let system_program = next_account_info(iter)?;

    let asset_id = read_u64(data)?;
    let policy = read_u8(data)?;
    let domain = read_u8(data)?;
    if policy > POLICY_WITH_SURPLUS || domain > DOMAIN_BACKING || !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !payer.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *system_program.key != solana_program::system_program::ID {
        return Err(ProgramError::IncorrectProgramId);
    }

    // Own-vault pools have no percolator market, so the market-binding seed components
    // are the default key (matching what the Pool stores below).
    let no_market = Pubkey::default();
    let asset_id_bytes = asset_id.to_le_bytes();
    let (expected_pool, bump) = Pubkey::find_program_address(
        &pool_seeds(mint.key, &asset_id_bytes, &no_market, &no_market),
        program_id,
    );
    if *pool_account.key != expected_pool {
        return Err(ProgramError::InvalidSeeds);
    }
    if pool_account.lamports() != 0 || pool_account.data_len() != 0 {
        return Err(ProgramError::AccountAlreadyInitialized);
    }

    // The vault must be an SPL token account for `mint`, whose authority is the
    // pool PDA — so only this program (signing as the pool) can move funds out.
    let vault_state = spl_token::state::Account::unpack(&vault.try_borrow_data()?)?;
    if vault_state.mint != *mint.key || vault_state.owner != expected_pool {
        return Err(ProgramError::InvalidAccountData);
    }

    let rent = solana_program::rent::Rent::get()?;
    let lamports = rent.minimum_balance(POOL_SIZE);
    let bump_arr = [bump];
    let seeds: [&[u8]; 6] = [
        b"subledger_pool",
        mint.key.as_ref(),
        &asset_id_bytes,
        no_market.as_ref(),
        no_market.as_ref(),
        &bump_arr,
    ];
    invoke_signed(
        &system_instruction::create_account(
            payer.key,
            pool_account.key,
            lamports,
            POOL_SIZE as u64,
            program_id,
        ),
        &[payer.clone(), pool_account.clone(), system_program.clone()],
        &[&seeds],
    )?;

    let pool = Pool {
        mint: *mint.key,
        asset_id,
        vault: *vault.key,
        outstanding_principal: 0,
        policy,
        domain,
        bump,
        market_slab: Pubkey::default(),
        percolator_program: Pubkey::default(),
        vote_authority: Pubkey::default(),
    };
    pool.serialize(&mut pool_account.try_borrow_mut_data()?);
    Ok(())
}

// deposit accounts: [owner(s,w), pool(w), position(w,pda), owner_ata(w), vault(w),
//                    token_program, system_program]
// data: amount (u64)
fn process_deposit(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &mut &[u8],
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let owner = next_account_info(iter)?;
    let pool_account = next_account_info(iter)?;
    let position_account = next_account_info(iter)?;
    let owner_ata = next_account_info(iter)?;
    let vault = next_account_info(iter)?;
    let token_program = next_account_info(iter)?;
    let system_program = next_account_info(iter)?;

    let amount = read_u64(data)?;
    if amount == 0 || !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !owner.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *token_program.key != spl_token::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    if pool_account.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    let mut pool = Pool::deserialize(&pool_account.try_borrow_data()?)?;
    // Type guard: the own-vault path must NOT touch an insurance pool. An
    // insurance pool's `vault` is the percolator insurance vault (owned by the
    // percolator vault_authority, not this pool PDA). An own-vault deposit here
    // would push funds into that vault WITHOUT a TopUpInsurance CPI — percolator
    // never counts them — and the own-vault withdraw could never sign them back
    // out, stranding the depositor's funds. Insurance pools use tags 4/5 only.
    if pool.is_insurance() {
        return Err(ProgramError::InvalidAccountData);
    }
    if *vault.key != pool.vault {
        return Err(ProgramError::InvalidAccountData);
    }

    // Position PDA (one per owner per pool).
    let pos_seeds = position_seeds(pool_account.key, owner.key);
    let (expected_pos, pos_bump) = Pubkey::find_program_address(&pos_seeds, program_id);
    if *position_account.key != expected_pos {
        return Err(ProgramError::InvalidSeeds);
    }
    let mut position = if position_account.data_len() == 0 || position_account.lamports() == 0 {
        let rent = solana_program::rent::Rent::get()?;
        let lamports = rent.minimum_balance(POSITION_SIZE);
        let bump_arr = [pos_bump];
        let seeds: [&[u8]; 4] = [
            b"subledger_position",
            pool_account.key.as_ref(),
            owner.key.as_ref(),
            &bump_arr,
        ];
        invoke_signed(
            &system_instruction::create_account(
                owner.key,
                position_account.key,
                lamports,
                POSITION_SIZE as u64,
                program_id,
            ),
            &[owner.clone(), position_account.clone(), system_program.clone()],
            &[&seeds],
        )?;
        Position {
            pool: *pool_account.key,
            owner: *owner.key,
            principal: 0,
            withdrawn_amount: 0,
            withdrawn: false,
            start_slot: 0,
            vote_locked: false,
        }
    } else {
        if position_account.owner != program_id {
            return Err(ProgramError::IllegalOwner);
        }
        let p = Position::deserialize(&position_account.try_borrow_data()?)?;
        if p.owner != *owner.key || p.pool != *pool_account.key {
            return Err(ProgramError::InvalidAccountData);
        }
        if p.withdrawn {
            return Err(ProgramError::InvalidAccountData);
        }
        p
    };

    // Pull principal into the vault (owner-signed).
    invoke(
        &spl_token::instruction::transfer(
            token_program.key,
            owner_ata.key,
            vault.key,
            owner.key,
            &[],
            amount,
        )?,
        &[owner_ata.clone(), vault.clone(), owner.clone(), token_program.clone()],
    )?;

    pool.outstanding_principal = pool
        .outstanding_principal
        .checked_add(amount)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    position.principal = position
        .principal
        .checked_add(amount)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    // Last-write-time: topping up resets the vote clock, so late additions don't
    // earn early-join weight.
    position.start_slot = Clock::get()?.slot;

    pool.serialize(&mut pool_account.try_borrow_mut_data()?);
    position.serialize(&mut position_account.try_borrow_mut_data()?);
    Ok(())
}

// withdraw accounts: [owner(s,w), pool(w), position(w), owner_ata(w), vault(w), token_program]
// data: none
fn process_withdraw(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &mut &[u8],
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let owner = next_account_info(iter)?;
    let pool_account = next_account_info(iter)?;
    let position_account = next_account_info(iter)?;
    let owner_ata = next_account_info(iter)?;
    let vault = next_account_info(iter)?;
    let token_program = next_account_info(iter)?;

    if !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !owner.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *token_program.key != spl_token::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    if pool_account.owner != program_id || position_account.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    let mut pool = Pool::deserialize(&pool_account.try_borrow_data()?)?;
    let mut position = Position::deserialize(&position_account.try_borrow_data()?)?;
    // Type guard: own-vault withdraw must never run against an insurance pool
    // (its vault is the percolator insurance vault; the pool PDA is not its token
    // authority, so this would fail anyway — reject early and explicitly). See
    // the matching guard in the own-vault deposit. Insurance uses tags 4/5.
    if pool.is_insurance() {
        return Err(ProgramError::InvalidAccountData);
    }

    // Re-derive the pool PDA so the recorded vault and signing seeds are trusted.
    // (own-vault: market_slab/percolator_program are the default key it stored.)
    let asset_id_bytes = pool.asset_id.to_le_bytes();
    let (expected_pool, bump) = Pubkey::find_program_address(
        &pool_seeds(&pool.mint, &asset_id_bytes, &pool.market_slab, &pool.percolator_program),
        program_id,
    );
    if *pool_account.key != expected_pool || bump != pool.bump {
        return Err(ProgramError::InvalidSeeds);
    }
    if *vault.key != pool.vault {
        return Err(ProgramError::InvalidAccountData);
    }
    // Owner-bound: only the position owner can exit, exactly once.
    if position.owner != *owner.key || position.pool != *pool_account.key {
        return Err(ProgramError::IllegalOwner);
    }
    if position.withdrawn || position.principal == 0 {
        return Err(ProgramError::InvalidAccountData);
    }
    if pool.outstanding_principal == 0 || position.principal > pool.outstanding_principal {
        return Err(ProgramError::InvalidAccountData);
    }

    let balance = token_balance(vault)?;
    let paid = payout(pool.policy, balance, pool.outstanding_principal, position.principal)?;

    if paid > 0 {
        let bump_arr = [pool.bump];
        let seeds: [&[u8]; 6] = [
            b"subledger_pool",
            pool.mint.as_ref(),
            &asset_id_bytes,
            pool.market_slab.as_ref(),
            pool.percolator_program.as_ref(),
            &bump_arr,
        ];
        invoke_signed(
            &spl_token::instruction::transfer(
                token_program.key,
                vault.key,
                owner_ata.key,
                pool_account.key,
                &[],
                paid,
            )?,
            &[vault.clone(), owner_ata.clone(), pool_account.clone(), token_program.clone()],
            &[&seeds],
        )?;
    }

    // A zero-payout exit still retires the position so an impaired/empty pool
    // cannot be replayed to distort other depositors' outstanding accounting.
    pool.outstanding_principal -= position.principal;
    position.withdrawn = true;
    position.withdrawn_amount = paid;

    pool.serialize(&mut pool_account.try_borrow_mut_data()?);
    position.serialize(&mut position_account.try_borrow_mut_data()?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Percolator-insurance pools
// ---------------------------------------------------------------------------
//
// A pool whose `vault` is the Percolator market's canonical insurance vault. The
// pool PDA is the asset-0 insurance *authority* (so it may TopUpInsurance) and the
// asset-0 insurance *operator* (so it may WithdrawInsuranceLimited). Principal is
// custodied by Percolator, never by this program; the only way out is the
// owner-authorized, principal-only exit, capped at the owner's own recorded
// principal — the pool can never take a depositor's funds.

fn perc_vault_authority(market_slab: &Pubkey, percolator_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"vault", market_slab.as_ref()], percolator_program).0
}

// init_insurance_pool accounts: [payer(s,w), mint, pool(w,pda), percolator_vault,
//   market_slab, percolator_program, system_program, vote_authority]
// data: asset_id (u64), policy (u8)
//
// `vote_authority` is the genesis-vote config PDA permitted to toggle a position's
// vote-lock (Pubkey::default() to disable). It is recorded as-is, not validated
// here — it only ever grants the right to BLOCK a withdrawal (set the lock), never
// to move funds, and the owner can always clear it by retracting the vote.
//
// `percolator_vault` must be the canonical insurance vault token account for
// `market_slab` (the ATA of its vault_authority), owned by the vault_authority PDA.
fn process_init_insurance_pool(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &mut &[u8],
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let mint = next_account_info(iter)?;
    let pool_account = next_account_info(iter)?;
    let percolator_vault = next_account_info(iter)?;
    let market_slab = next_account_info(iter)?;
    let percolator_program = next_account_info(iter)?;
    let system_program = next_account_info(iter)?;
    let vote_authority = next_account_info(iter)?;

    let asset_id = read_u64(data)?;
    let policy = read_u8(data)?;
    if policy > POLICY_WITH_SURPLUS || !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !payer.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *system_program.key != solana_program::system_program::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    if *percolator_program.key == Pubkey::default() {
        return Err(ProgramError::InvalidAccountData);
    }

    let asset_id_bytes = asset_id.to_le_bytes();
    let (expected_pool, bump) = Pubkey::find_program_address(
        &pool_seeds(mint.key, &asset_id_bytes, market_slab.key, percolator_program.key),
        program_id,
    );
    if *pool_account.key != expected_pool {
        return Err(ProgramError::InvalidSeeds);
    }
    if pool_account.lamports() != 0 || pool_account.data_len() != 0 {
        return Err(ProgramError::AccountAlreadyInitialized);
    }

    // The vault is the Percolator canonical insurance vault: an SPL token account
    // for `mint`, owned by the market's vault_authority PDA.
    let vault_authority = perc_vault_authority(market_slab.key, percolator_program.key);
    let vault_state = spl_token::state::Account::unpack(&percolator_vault.try_borrow_data()?)?;
    if vault_state.mint != *mint.key || vault_state.owner != vault_authority {
        return Err(ProgramError::InvalidAccountData);
    }
    // Pin to the single canonical vault address Percolator enforces (F-VAULT-FRAG),
    // not merely "some vault_authority-owned token account". Binding a pool to a
    // non-canonical vault would leave it inert (every deposit/withdraw CPI reverts
    // with InvalidVaultAccount); reject it up front. Closes issue #24 on the
    // active path (PR #25 only covered the deprecated custodial program/).
    if *percolator_vault.key != canonical_vault_address(&vault_authority, mint.key) {
        return Err(ProgramError::InvalidAccountData);
    }

    let rent = solana_program::rent::Rent::get()?;
    let lamports = rent.minimum_balance(POOL_SIZE);
    let bump_arr = [bump];
    let seeds: [&[u8]; 6] = [
        b"subledger_pool",
        mint.key.as_ref(),
        &asset_id_bytes,
        market_slab.key.as_ref(),
        percolator_program.key.as_ref(),
        &bump_arr,
    ];
    invoke_signed(
        &system_instruction::create_account(
            payer.key,
            pool_account.key,
            lamports,
            POOL_SIZE as u64,
            program_id,
        ),
        &[payer.clone(), pool_account.clone(), system_program.clone()],
        &[&seeds],
    )?;

    let pool = Pool {
        mint: *mint.key,
        asset_id,
        vault: *percolator_vault.key,
        outstanding_principal: 0,
        policy,
        domain: DOMAIN_INSURANCE,
        bump,
        market_slab: *market_slab.key,
        percolator_program: *percolator_program.key,
        vote_authority: *vote_authority.key,
    };
    pool.serialize(&mut pool_account.try_borrow_mut_data()?);
    Ok(())
}

// insurance_deposit accounts: [owner(s,w), pool(w), position(w,pda), owner_ata(w),
//   holding(w, pool-PDA-owned token acct), market_slab(w), percolator_vault(w),
//   percolator_program, token_program, system_program]
// data: amount (u64)
//
// User -> holding (user-signed). Then the pool PDA (asset-0 insurance authority)
// signs TopUpInsurance moving holding -> Percolator insurance vault. Records the
// position (principal += amount, start_slot = now) and bumps outstanding.
fn process_insurance_deposit(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &mut &[u8],
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let owner = next_account_info(iter)?;
    let pool_account = next_account_info(iter)?;
    let position_account = next_account_info(iter)?;
    let owner_ata = next_account_info(iter)?;
    let holding = next_account_info(iter)?;
    let market_slab = next_account_info(iter)?;
    let percolator_vault = next_account_info(iter)?;
    let percolator_program = next_account_info(iter)?;
    let token_program = next_account_info(iter)?;
    let system_program = next_account_info(iter)?;

    let amount = read_u64(data)?;
    if amount == 0 || !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !owner.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *token_program.key != spl_token::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    if pool_account.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    let mut pool = Pool::deserialize(&pool_account.try_borrow_data()?)?;
    if !pool.is_insurance() {
        return Err(ProgramError::InvalidAccountData);
    }
    // Re-derive the pool PDA so the signing seeds are trusted.
    let asset_id_bytes = pool.asset_id.to_le_bytes();
    let (expected_pool, bump) = Pubkey::find_program_address(
        &pool_seeds(&pool.mint, &asset_id_bytes, &pool.market_slab, &pool.percolator_program),
        program_id,
    );
    if *pool_account.key != expected_pool || bump != pool.bump {
        return Err(ProgramError::InvalidSeeds);
    }
    if *market_slab.key != pool.market_slab
        || *percolator_vault.key != pool.vault
        || *percolator_program.key != pool.percolator_program
    {
        return Err(ProgramError::InvalidAccountData);
    }

    // Position PDA (one per owner per pool).
    let pos_seeds = position_seeds(pool_account.key, owner.key);
    let (expected_pos, pos_bump) = Pubkey::find_program_address(&pos_seeds, program_id);
    if *position_account.key != expected_pos {
        return Err(ProgramError::InvalidSeeds);
    }
    let mut position = if position_account.data_len() == 0 || position_account.lamports() == 0 {
        let rent = solana_program::rent::Rent::get()?;
        let lamports = rent.minimum_balance(POSITION_SIZE);
        let pbump = [pos_bump];
        let seeds: [&[u8]; 4] = [
            b"subledger_position",
            pool_account.key.as_ref(),
            owner.key.as_ref(),
            &pbump,
        ];
        invoke_signed(
            &system_instruction::create_account(
                owner.key,
                position_account.key,
                lamports,
                POSITION_SIZE as u64,
                program_id,
            ),
            &[owner.clone(), position_account.clone(), system_program.clone()],
            &[&seeds],
        )?;
        Position {
            pool: *pool_account.key,
            owner: *owner.key,
            principal: 0,
            withdrawn_amount: 0,
            withdrawn: false,
            start_slot: 0,
            vote_locked: false,
        }
    } else {
        if position_account.owner != program_id {
            return Err(ProgramError::IllegalOwner);
        }
        let p = Position::deserialize(&position_account.try_borrow_data()?)?;
        if p.owner != *owner.key || p.pool != *pool_account.key || p.withdrawn {
            return Err(ProgramError::InvalidAccountData);
        }
        p
    };

    // 1) User -> holding (user-signed; the user is moving their own funds).
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

    // 2) holding -> Percolator insurance vault, signed by the pool PDA as the
    //    asset-0 insurance authority (TopUpInsurance, tag 9).
    let seeds: [&[u8]; 6] = [
        b"subledger_pool",
        pool.mint.as_ref(),
        &asset_id_bytes,
        pool.market_slab.as_ref(),
        pool.percolator_program.as_ref(),
        core::slice::from_ref(&pool.bump),
    ];
    let mut ix_data = vec![PERC_IX_TOP_UP_INSURANCE];
    ix_data.extend_from_slice(&(amount as u128).to_le_bytes());
    invoke_signed(
        &Instruction {
            program_id: *percolator_program.key,
            accounts: vec![
                AccountMeta::new_readonly(*pool_account.key, true),
                AccountMeta::new(*market_slab.key, false),
                AccountMeta::new(*holding.key, false),
                AccountMeta::new(*percolator_vault.key, false),
                AccountMeta::new_readonly(*token_program.key, false),
            ],
            data: ix_data,
        },
        &[
            pool_account.clone(),
            market_slab.clone(),
            holding.clone(),
            percolator_vault.clone(),
            token_program.clone(),
            percolator_program.clone(),
        ],
        &[&seeds],
    )?;

    pool.outstanding_principal = pool
        .outstanding_principal
        .checked_add(amount)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    position.principal = position
        .principal
        .checked_add(amount)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    // Last-write-time: topping up resets the vote clock.
    position.start_slot = Clock::get()?.slot;

    pool.serialize(&mut pool_account.try_borrow_mut_data()?);
    position.serialize(&mut position_account.try_borrow_mut_data()?);
    Ok(())
}

// insurance_withdraw accounts: [owner(s,w), pool(w), position(w), owner_ata(w),
//   holding(w, pool-PDA-owned token acct), market_slab(w), percolator_vault(w),
//   vault_authority, percolator_program, token_program]
// data: amount (u64)
//
// Owner-bound, principal-only exit: `amount <= position.principal`. The pool PDA
// (asset-0 insurance operator) signs WithdrawInsuranceLimited (tag 23). NOTE: the
// real percolator handler requires the withdraw destination to be owned by the
// *operator* (the pool PDA), not an arbitrary user, so we withdraw into a
// pool-PDA-owned holding account and then SPL-transfer holding -> owner's ATA
// (pool PDA signs). Can never exceed the owner's own recorded principal.
fn process_insurance_withdraw(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &mut &[u8],
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let owner = next_account_info(iter)?;
    let pool_account = next_account_info(iter)?;
    let position_account = next_account_info(iter)?;
    let owner_ata = next_account_info(iter)?;
    let holding = next_account_info(iter)?;
    let market_slab = next_account_info(iter)?;
    let percolator_vault = next_account_info(iter)?;
    let vault_authority = next_account_info(iter)?;
    let percolator_program = next_account_info(iter)?;
    let token_program = next_account_info(iter)?;

    let amount = read_u64(data)?;
    if amount == 0 || !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !owner.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *token_program.key != spl_token::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    if pool_account.owner != program_id || position_account.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    let mut pool = Pool::deserialize(&pool_account.try_borrow_data()?)?;
    let mut position = Position::deserialize(&position_account.try_borrow_data()?)?;
    if !pool.is_insurance() {
        return Err(ProgramError::InvalidAccountData);
    }

    let asset_id_bytes = pool.asset_id.to_le_bytes();
    let (expected_pool, bump) = Pubkey::find_program_address(
        &pool_seeds(&pool.mint, &asset_id_bytes, &pool.market_slab, &pool.percolator_program),
        program_id,
    );
    if *pool_account.key != expected_pool || bump != pool.bump {
        return Err(ProgramError::InvalidSeeds);
    }
    if *market_slab.key != pool.market_slab
        || *percolator_vault.key != pool.vault
        || *percolator_program.key != pool.percolator_program
    {
        return Err(ProgramError::InvalidAccountData);
    }
    // vault_authority is a passed account, validated by PDA derivation.
    if *vault_authority.key != perc_vault_authority(market_slab.key, percolator_program.key) {
        return Err(ProgramError::InvalidSeeds);
    }
    // The holding account must be a token account for `mint` owned by the pool PDA
    // (the real percolator handler requires the withdraw dest to be the operator).
    let holding_state = spl_token::state::Account::unpack(&holding.try_borrow_data()?)?;
    if holding_state.mint != pool.mint || holding_state.owner != *pool_account.key {
        return Err(ProgramError::InvalidAccountData);
    }
    // Owner-bound: only the position owner can exit.
    if position.owner != *owner.key || position.pool != *pool_account.key {
        return Err(ProgramError::IllegalOwner);
    }
    if position.withdrawn {
        return Err(ProgramError::InvalidAccountData);
    }
    // Vote-locked: the principal is pledged to a live genesis vote. The owner must
    // retract that vote first (which clears the lock). This keeps the vote's
    // principal snapshot backed by capital that is still at risk — without it a
    // voter could exit and leave a free, capital-less ballot inflating quorum.
    if position.vote_locked {
        // Vote-locked: retract the genesis vote first (which clears the lock).
        return Err(ProgramError::InvalidAccountData);
    }
    // Principal-only: never exceeds the owner's own recorded principal.
    if amount > position.principal || amount > pool.outstanding_principal {
        return Err(ProgramError::InsufficientFunds);
    }

    // The pool PDA (asset-0 insurance operator) signs WithdrawInsuranceLimited,
    // moving Percolator insurance -> pool-PDA-owned holding.
    let seeds: [&[u8]; 6] = [
        b"subledger_pool",
        pool.mint.as_ref(),
        &asset_id_bytes,
        pool.market_slab.as_ref(),
        pool.percolator_program.as_ref(),
        core::slice::from_ref(&pool.bump),
    ];
    let mut ix_data = vec![PERC_IX_WITHDRAW_INSURANCE_LIMITED];
    ix_data.extend_from_slice(&(amount as u128).to_le_bytes());
    invoke_signed(
        &Instruction {
            program_id: *percolator_program.key,
            accounts: vec![
                AccountMeta::new_readonly(*pool_account.key, true),
                AccountMeta::new(*market_slab.key, false),
                AccountMeta::new(*holding.key, false),
                AccountMeta::new(*percolator_vault.key, false),
                AccountMeta::new_readonly(*vault_authority.key, false),
                AccountMeta::new_readonly(*token_program.key, false),
            ],
            data: ix_data,
        },
        &[
            pool_account.clone(),
            market_slab.clone(),
            holding.clone(),
            percolator_vault.clone(),
            vault_authority.clone(),
            token_program.clone(),
            percolator_program.clone(),
        ],
        &[&seeds],
    )?;

    // holding -> owner's ATA, signed by the pool PDA. This is the only path out and
    // it is bounded by the owner's recorded principal, so the program can never pay
    // the owner more than they put in.
    invoke_signed(
        &spl_token::instruction::transfer(
            token_program.key,
            holding.key,
            owner_ata.key,
            pool_account.key,
            &[],
            amount,
        )?,
        &[holding.clone(), owner_ata.clone(), pool_account.clone(), token_program.clone()],
        &[&seeds],
    )?;

    pool.outstanding_principal -= amount;
    position.principal -= amount;
    position.withdrawn_amount = position
        .withdrawn_amount
        .checked_add(amount)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    if position.principal == 0 {
        position.withdrawn = true;
    }

    pool.serialize(&mut pool_account.try_borrow_mut_data()?);
    position.serialize(&mut position_account.try_borrow_mut_data()?);
    Ok(())
}

// set_vote_lock accounts: [vote_authority(signer), pool, position(w), owner(signer)]
// data: locked (u8) — 1 lock, 0 unlock
//
// Toggles a position's vote-lock. ONLY the pool's registered vote_authority (the
// genesis-vote config PDA) may call it, and only on an insurance pool. This grants
// the genesis vote the right to BLOCK a withdrawal while a ballot is live — never
// to move funds. The owner always retains the ability to clear it by retracting
// their vote, so funds can never be permanently frozen by this mechanism.
fn process_set_vote_lock(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &mut &[u8],
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let vote_authority = next_account_info(iter)?;
    let pool_account = next_account_info(iter)?;
    let position_account = next_account_info(iter)?;
    let owner = next_account_info(iter)?;

    let locked = read_u8(data)?;
    if locked > 1 || !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !vote_authority.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    // The position OWNER must also sign. Without this, an attacker who front-runs
    // pool init with an attacker-controlled vote_authority could lock any
    // depositor's position and freeze their withdrawal forever. Requiring the
    // owner's signature means a position can only ever be (un)locked in the context
    // of the owner acting on their OWN vote — which is the only legitimate case.
    // The vote_authority gate stays so the owner cannot self-unlock to bypass
    // retract (that would re-open the vote-outlives-capital hole).
    if !owner.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if pool_account.owner != program_id || position_account.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    let pool = Pool::deserialize(&pool_account.try_borrow_data()?)?;
    // Vote-locking is only meaningful for the insurance vote-bond pool, and only the
    // registered authority may toggle it. A default authority means locking is off.
    if !pool.is_insurance()
        || pool.vote_authority == Pubkey::default()
        || pool.vote_authority != *vote_authority.key
    {
        return Err(ProgramError::IllegalOwner);
    }
    let mut position = Position::deserialize(&position_account.try_borrow_data()?)?;
    if position.pool != *pool_account.key || position.owner != *owner.key {
        return Err(ProgramError::InvalidAccountData);
    }
    position.vote_locked = locked == 1;
    position.serialize(&mut position_account.try_borrow_mut_data()?);
    Ok(())
}

// accept_operator accounts: [asset_admin(signer), pool, market_slab(w), percolator_program]
// data: none
//
// This is NOT a key-rotation instruction and gives the subledger no power over keys.
// Squads is the asset_admin and the ONLY party that rotates the percolator operator; the
// subledger merely supplies the pool's mandatory incoming CONSENT so a Squads-initiated
// grant can land. percolator's UpdateAssetAuthority requires a non-zero incoming key to
// co-sign (asset-0 insurance has no consent-free grant path), and a PDA can only co-sign
// via its program — so without this one-line consent hook the Squads grant cannot complete.
// It is deliberately powerless: it hardcodes the new operator/authority to THIS pool's own
// PDA (never an arbitrary key), and it only succeeds when the real asset_admin (the Squads
// vault, reachable only via a timelock'd execute) co-signs — percolator enforces that.
// So: Squads rotates the key; the subledger only says "yes, I will hold the funds." The
// granted asset-0 insurance authority (kind 1, gates TopUpInsurance) + operator (kind 2,
// gates WithdrawInsuranceLimited) are what let insurance_deposit/withdraw sign as the pool
// during genesis. Later the DAO, via Squads, rotates the operator onward to the twap (whose
// own accept_operator is the exact mirror of this). Safe to leave permissionless: it can
// only ever make the canonical, market-bound pool the operator, and only with Squads' sig.
fn process_accept_operator(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &mut &[u8],
) -> ProgramResult {
    if !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    let iter = &mut accounts.iter();
    let asset_admin = next_account_info(iter)?; // the market's current asset_admin (Squads vault)
    let pool_account = next_account_info(iter)?;
    let market_slab = next_account_info(iter)?;
    let percolator_program = next_account_info(iter)?;

    if !asset_admin.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if pool_account.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    let pool = Pool::deserialize(&pool_account.try_borrow_data()?)?;
    if !pool.is_insurance() {
        return Err(ProgramError::InvalidAccountData);
    }
    if *market_slab.key != pool.market_slab || *percolator_program.key != pool.percolator_program {
        return Err(ProgramError::InvalidAccountData);
    }
    // Re-derive the pool PDA so the signing seeds are trusted.
    let asset_id_bytes = pool.asset_id.to_le_bytes();
    let (expected_pool, bump) = Pubkey::find_program_address(
        &pool_seeds(&pool.mint, &asset_id_bytes, &pool.market_slab, &pool.percolator_program),
        program_id,
    );
    if *pool_account.key != expected_pool || bump != pool.bump {
        return Err(ProgramError::InvalidSeeds);
    }

    let seeds: [&[u8]; 6] = [
        b"subledger_pool",
        pool.mint.as_ref(),
        &asset_id_bytes,
        pool.market_slab.as_ref(),
        pool.percolator_program.as_ref(),
        core::slice::from_ref(&pool.bump),
    ];
    // Receive BOTH the insurance authority (TopUp) and operator (Withdraw) roles for
    // asset 0. percolator requires the current asset_admin (asset_admin) and the incoming
    // key (the pool) to co-sign each rotation.
    for kind in [ASSET_AUTH_INSURANCE, ASSET_AUTH_INSURANCE_OPERATOR] {
        let mut ix_data = vec![PERC_IX_UPDATE_ASSET_AUTHORITY];
        ix_data.extend_from_slice(&0u16.to_le_bytes()); // asset_index 0
        ix_data.push(kind);
        ix_data.extend_from_slice(pool_account.key.as_ref()); // new authority = the pool itself
        invoke_signed(
            &Instruction {
                program_id: *percolator_program.key,
                accounts: vec![
                    AccountMeta::new_readonly(*asset_admin.key, true), // current asset_admin
                    AccountMeta::new_readonly(*pool_account.key, true), // new (the pool co-signs)
                    AccountMeta::new(*market_slab.key, false),
                ],
                data: ix_data,
            },
            &[
                asset_admin.clone(),
                pool_account.clone(),
                market_slab.clone(),
                percolator_program.clone(),
            ],
            &[&seeds],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests for the pure payout arithmetic
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn principal_policy_healthy_pays_principal_keeps_surplus() {
        // balance 150 >= outstanding 100: each principal-100 exit gets exactly principal.
        assert_eq!(payout(POLICY_PRINCIPAL, 150, 100, 40).unwrap(), 40);
        assert_eq!(payout(POLICY_PRINCIPAL, 150, 100, 60).unwrap(), 60);
    }

    #[test]
    fn principal_policy_impaired_is_pro_rata() {
        // balance 50 < outstanding 100: pro-rata haircut.
        assert_eq!(payout(POLICY_PRINCIPAL, 50, 100, 40).unwrap(), 20);
        assert_eq!(payout(POLICY_PRINCIPAL, 50, 100, 60).unwrap(), 30);
    }

    #[test]
    fn with_surplus_returns_yield_pro_rata() {
        // balance 150, outstanding 100: surplus 50 distributed pro-rata.
        assert_eq!(payout(POLICY_WITH_SURPLUS, 150, 100, 40).unwrap(), 60);
        assert_eq!(payout(POLICY_WITH_SURPLUS, 150, 100, 60).unwrap(), 90);
    }

    #[test]
    fn rejects_degenerate_inputs() {
        assert!(payout(POLICY_PRINCIPAL, 100, 0, 10).is_err());
        assert!(payout(POLICY_PRINCIPAL, 100, 100, 0).is_err());
        assert!(payout(POLICY_PRINCIPAL, 100, 100, 101).is_err());
    }

    #[test]
    fn state_round_trips() {
        let slab = Pubkey::new_unique();
        let perc = Pubkey::new_unique();
        let pool = Pool {
            mint: Pubkey::new_unique(),
            asset_id: 7,
            vault: Pubkey::new_unique(),
            outstanding_principal: 12345,
            policy: POLICY_WITH_SURPLUS,
            domain: DOMAIN_BACKING,
            bump: 254,
            market_slab: slab,
            percolator_program: perc,
            vote_authority: Pubkey::new_unique(),
        };
        let mut buf = [0u8; POOL_SIZE];
        pool.serialize(&mut buf);
        let d = Pool::deserialize(&buf).unwrap();
        assert_eq!(d.mint, pool.mint);
        assert_eq!(d.asset_id, 7);
        assert_eq!(d.vault, pool.vault);
        assert_eq!(d.outstanding_principal, 12345);
        assert_eq!(d.policy, POLICY_WITH_SURPLUS);
        assert_eq!(d.domain, DOMAIN_BACKING);
        assert_eq!(d.bump, 254);
        assert_eq!(d.market_slab, slab);
        assert_eq!(d.percolator_program, perc);
        assert_eq!(d.vote_authority, pool.vote_authority);
        assert!(d.is_insurance());

        let pos = Position {
            pool: Pubkey::new_unique(),
            owner: Pubkey::new_unique(),
            principal: 999,
            withdrawn_amount: 111,
            withdrawn: true,
            start_slot: 4242,
            vote_locked: true,
        };
        let mut pbuf = [0u8; POSITION_SIZE];
        pos.serialize(&mut pbuf);
        let dp = Position::deserialize(&pbuf).unwrap();
        assert_eq!(dp.owner, pos.owner);
        assert_eq!(dp.principal, 999);
        assert!(dp.withdrawn);
        assert_eq!(dp.start_slot, 4242);
        assert!(dp.vote_locked);
    }
}
