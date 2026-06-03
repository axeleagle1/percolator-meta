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
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    declare_id,
    entrypoint::ProgramResult,
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
const POOL_SIZE: usize = 96; // 8+32+8+32+8+1+1 padded
const POSITION_SIZE: usize = 96; // 8+32+32+8+8+1 padded

const POLICY_PRINCIPAL: u8 = 0;
const POLICY_WITH_SURPLUS: u8 = 1;

const IX_INIT_POOL: u8 = 0;
const IX_DEPOSIT: u8 = 1;
const IX_WITHDRAW: u8 = 2;

#[cfg(not(feature = "no-entrypoint"))]
solana_program::entrypoint!(process_instruction);

fn pool_seeds<'a>(mint: &'a Pubkey, asset_id: &'a [u8; 8]) -> [&'a [u8]; 3] {
    [b"subledger_pool", mint.as_ref(), asset_id]
}

fn position_seeds<'a>(pool: &'a Pubkey, owner: &'a Pubkey) -> [&'a [u8]; 3] {
    [b"subledger_position", pool.as_ref(), owner.as_ref()]
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct Pool {
    mint: Pubkey,
    asset_id: u64,
    vault: Pubkey,
    outstanding_principal: u64,
    policy: u8,
    bump: u8,
}

impl Pool {
    fn deserialize(data: &[u8]) -> Result<Self, ProgramError> {
        if data.len() < POOL_SIZE || data[..8] != POOL_DISC {
            return Err(ProgramError::InvalidAccountData);
        }
        let policy = data[88];
        if policy > POLICY_WITH_SURPLUS {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(Self {
            mint: Pubkey::new_from_array(data[8..40].try_into().unwrap()),
            asset_id: u64::from_le_bytes(data[40..48].try_into().unwrap()),
            vault: Pubkey::new_from_array(data[48..80].try_into().unwrap()),
            outstanding_principal: u64::from_le_bytes(data[80..88].try_into().unwrap()),
            policy,
            bump: data[89],
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
        data[90..POOL_SIZE].fill(0);
    }
}

struct Position {
    pool: Pubkey,
    owner: Pubkey,
    principal: u64,
    withdrawn_amount: u64,
    withdrawn: bool,
}

impl Position {
    fn deserialize(data: &[u8]) -> Result<Self, ProgramError> {
        if data.len() < POSITION_SIZE || data[..8] != POSITION_DISC {
            return Err(ProgramError::InvalidAccountData);
        }
        let withdrawn = data[88];
        if withdrawn > 1 {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(Self {
            pool: Pubkey::new_from_array(data[8..40].try_into().unwrap()),
            owner: Pubkey::new_from_array(data[40..72].try_into().unwrap()),
            principal: u64::from_le_bytes(data[72..80].try_into().unwrap()),
            withdrawn_amount: u64::from_le_bytes(data[80..88].try_into().unwrap()),
            withdrawn: withdrawn == 1,
        })
    }

    fn serialize(&self, data: &mut [u8]) {
        data[..8].copy_from_slice(&POSITION_DISC);
        data[8..40].copy_from_slice(self.pool.as_ref());
        data[40..72].copy_from_slice(self.owner.as_ref());
        data[72..80].copy_from_slice(&self.principal.to_le_bytes());
        data[80..88].copy_from_slice(&self.withdrawn_amount.to_le_bytes());
        data[88] = self.withdrawn as u8;
        data[89..POSITION_SIZE].fill(0);
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
    if policy > POLICY_WITH_SURPLUS || !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !payer.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *system_program.key != solana_program::system_program::ID {
        return Err(ProgramError::IncorrectProgramId);
    }

    let asset_id_bytes = asset_id.to_le_bytes();
    let (expected_pool, bump) =
        Pubkey::find_program_address(&pool_seeds(mint.key, &asset_id_bytes), program_id);
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
    let seeds: [&[u8]; 4] = [b"subledger_pool", mint.key.as_ref(), &asset_id_bytes, &bump_arr];
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
        bump,
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

    // Re-derive the pool PDA so the recorded vault and signing seeds are trusted.
    let asset_id_bytes = pool.asset_id.to_le_bytes();
    let (expected_pool, bump) =
        Pubkey::find_program_address(&pool_seeds(&pool.mint, &asset_id_bytes), program_id);
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
        let seeds: [&[u8]; 4] = [
            b"subledger_pool",
            pool.mint.as_ref(),
            &asset_id_bytes,
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
        let pool = Pool {
            mint: Pubkey::new_unique(),
            asset_id: 7,
            vault: Pubkey::new_unique(),
            outstanding_principal: 12345,
            policy: POLICY_WITH_SURPLUS,
            bump: 254,
        };
        let mut buf = [0u8; POOL_SIZE];
        pool.serialize(&mut buf);
        let d = Pool::deserialize(&buf).unwrap();
        assert_eq!(d.mint, pool.mint);
        assert_eq!(d.asset_id, 7);
        assert_eq!(d.vault, pool.vault);
        assert_eq!(d.outstanding_principal, 12345);
        assert_eq!(d.policy, POLICY_WITH_SURPLUS);
        assert_eq!(d.bump, 254);

        let pos = Position {
            pool: Pubkey::new_unique(),
            owner: Pubkey::new_unique(),
            principal: 999,
            withdrawn_amount: 111,
            withdrawn: true,
        };
        let mut pbuf = [0u8; POSITION_SIZE];
        pos.serialize(&mut pbuf);
        let dp = Position::deserialize(&pbuf).unwrap();
        assert_eq!(dp.owner, pos.owner);
        assert_eq!(dp.principal, 999);
        assert!(dp.withdrawn);
    }
}
