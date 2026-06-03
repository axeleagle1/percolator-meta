use crate::{Amount, Slot};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MarketPair {
    pub base: &'static str,
    pub quote: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DefaultMarketRiskConfig {
    pub max_leverage: u64,
    pub initial_margin_bps: u64,
    pub max_accrual_dt_slots: u64,
}

pub const DEFAULT_STABLE_BASE_SYMBOL: &str = "USDC";
pub const DEFAULT_MARKET_0_BASE_SYMBOL: &str = "BTC";
pub const DEFAULT_MARKET_0_QUOTE_SYMBOL: &str = DEFAULT_STABLE_BASE_SYMBOL;
pub const USDC_DECIMALS: u8 = 6;
pub const USDC_ATOMS_PER_USDC: Amount = 1_000_000;
pub const DEFAULT_PERMISSIONLESS_MARKET_CREATION_FEE_USDC: Amount = 1_000;
pub const DEFAULT_PERMISSIONLESS_MARKET_CREATION_FEE_USDC_ATOMS: Amount =
    DEFAULT_PERMISSIONLESS_MARKET_CREATION_FEE_USDC * USDC_ATOMS_PER_USDC;
pub const DEFAULT_MAX_LEVERAGE: u64 = 20;
pub const DEFAULT_INITIAL_MARGIN_BPS: u64 = 10_000 / DEFAULT_MAX_LEVERAGE;
pub const DEFAULT_MAX_ACCRUAL_DT_SLOTS: u64 = 20;
pub const DEFAULT_SLOTS_PER_SECOND: Slot = 2;
pub const DEFAULT_SECONDS_PER_DAY: Slot = 86_400;
pub const DEFAULT_GENESIS_DAYS: Slot = 90;
pub const DEFAULT_GENESIS_PERIOD_SLOTS: Slot =
    DEFAULT_GENESIS_DAYS * DEFAULT_SECONDS_PER_DAY * DEFAULT_SLOTS_PER_SECOND;
pub const SOLANA_USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
pub const DEFAULT_MARKET_0_PAIR: MarketPair = MarketPair {
    base: DEFAULT_MARKET_0_BASE_SYMBOL,
    quote: DEFAULT_MARKET_0_QUOTE_SYMBOL,
};
pub const DEFAULT_MARKET_RISK_CONFIG: DefaultMarketRiskConfig = DefaultMarketRiskConfig {
    max_leverage: DEFAULT_MAX_LEVERAGE,
    initial_margin_bps: DEFAULT_INITIAL_MARGIN_BPS,
    max_accrual_dt_slots: DEFAULT_MAX_ACCRUAL_DT_SLOTS,
};

pub fn default_market_0_pair() -> MarketPair {
    DEFAULT_MARKET_0_PAIR
}

pub fn default_market_risk_config() -> DefaultMarketRiskConfig {
    DEFAULT_MARKET_RISK_CONFIG
}
