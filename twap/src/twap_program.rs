use crate::{Amount, Slot};
use solana_program::pubkey::Pubkey;

pub use crate::surplus::{
    associated_token_program_id, derive_associated_token_address, derive_bid_escrow_pda, BidBook,
    BidBookEntry, BidPlacement, BidRefundAtaSnapshot, BuyBurnExecution, BuyBurnFill, EvictedBid,
    Market0Insurance, Market0Surplus, PendingBurnBid, PermissionlessBuyBurnExecution,
    PermissionlessBuyBurnRequest, RegisteredBid, SurplusError, TwapAuthorityChain,
    TwapBuyBurnSchedule, TwapBuyBurnState, TwapProgramConfig, TwapProgramRotation,
    TwapWithdrawAccounts, ASSOCIATED_TOKEN_PROGRAM_ID, BID_ESCROW_SEED, BPS_DENOMINATOR,
    MARKET_0_SURPLUS_BUY_BURN_BPS, MARKET_0_SURPLUS_RETAIN_BPS, MAX_TWAP_BIDS_PER_EXECUTION,
    MAX_TWAP_INTERVAL_COUNT, TARGET_SLOTS_PER_SECOND, TWAP_AUTHORITY_SEED, TWAP_INTERVAL_COUNT,
    TWAP_INTERVAL_SECONDS, TWAP_INTERVAL_SLOTS, TWAP_TOTAL_SLOTS,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ReusableTwapProgram {
    pub config: TwapProgramConfig,
    pub state: TwapBuyBurnState,
}

impl ReusableTwapProgram {
    pub fn initialize_market_0_default(
        config: TwapProgramConfig,
        start_slot: Slot,
        insurance: Market0Insurance,
    ) -> Result<Self, SurplusError> {
        let schedule = config.market_0_default_schedule(start_slot, insurance)?;
        Self::initialize_with_schedule(config, schedule)
    }

    pub fn initialize_with_schedule(
        config: TwapProgramConfig,
        schedule: TwapBuyBurnSchedule,
    ) -> Result<Self, SurplusError> {
        if schedule.surplus_buy_burn_bps != config.surplus_buy_burn_bps {
            return Err(SurplusError::TwapPolicyMismatch);
        }
        Ok(Self {
            config,
            state: TwapBuyBurnState::new(schedule),
        })
    }

    pub fn accept_bids_permissionlessly(
        &mut self,
        request: PermissionlessBuyBurnRequest,
        insurance: &mut Market0Insurance,
        bids: &mut [RegisteredBid],
    ) -> Result<PermissionlessBuyBurnExecution, SurplusError> {
        self.config
            .accept_bids_permissionlessly(request, &mut self.state, insurance, bids)
    }

    pub fn rotate_twap_program(
        &mut self,
        controller: Pubkey,
        new_twap_program: Pubkey,
        new_withdraw_accounts: TwapWithdrawAccounts,
    ) -> Result<TwapProgramRotation, SurplusError> {
        self.config
            .rotate_twap_program(controller, new_twap_program, new_withdraw_accounts)
    }

    pub fn pulled_from_insurance(&self) -> Amount {
        self.state.pulled_from_insurance
    }

    pub fn burned_coin_atoms(&self) -> Amount {
        self.state.burned_coin_atoms
    }
}
