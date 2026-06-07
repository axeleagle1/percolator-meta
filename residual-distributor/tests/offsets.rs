//! [branch-only, DO NOT PUSH] Finding-T pins: the decider's hardcoded snapshot offsets MUST equal
//! HEADER_LEN + offset_of!(real percolator struct, field), and its subledger Position offsets MUST
//! equal the subledger's canonical layout. If percolator reorders BackingDomainLedgerAccountV16 or
//! the subledger reorders Position, these fail — preventing the GT/HF-class drift where a consumer
//! reads at stale offsets against a rebuilt dependency.

use core::mem::offset_of;
use percolator_prog::state::BackingDomainLedgerAccountV16 as B;
use percolator::PortfolioAccountV16Account as P;
use residual_distributor::{
    OFF_BACKING_AUTHORITY, OFF_BACKING_MARKET_GROUP, OFF_CUMULATIVE_LOSS, OFF_PORTFOLIO_CRYSTALLIZED_LOSS,
    OFF_PORTFOLIO_MARKET_GROUP, OFF_PORTFOLIO_OWNER, OFF_PORTFOLIO_RECEIVED, OFF_TOTAL_EARNINGS,
    OFF_TOTAL_PRINCIPAL, PERC_HEADER_LEN,
};

// LP & trader cohort counters live in PortfolioAccountV16Account (read at HEADER_LEN..). PINNED so a
// percolator reorder of the portfolio header can't silently shift the residual reward reads.
#[test]
fn portfolio_residual_counter_offsets_match_the_real_percolator_struct() {
    assert_eq!(
        OFF_PORTFOLIO_MARKET_GROUP,
        PERC_HEADER_LEN + offset_of!(P, provenance_header) + offset_of!(percolator::ProvenanceHeaderV16Account, market_group_id),
        "portfolio provenance market_group (LP/trader Pyth-market scope) offset"
    );
    assert_eq!(
        OFF_PORTFOLIO_OWNER,
        PERC_HEADER_LEN + offset_of!(P, owner),
        "portfolio owner (LP/trader reward owner) offset"
    );
    assert_eq!(
        OFF_PORTFOLIO_CRYSTALLIZED_LOSS,
        PERC_HEADER_LEN + offset_of!(P, residual_crystallized_loss_atoms_total),
        "trader cohort: crystallized-loss counter offset"
    );
    assert_eq!(
        OFF_PORTFOLIO_RECEIVED,
        PERC_HEADER_LEN + offset_of!(P, residual_received_atoms_total),
        "LP cohort: residual-received counter offset"
    );
}

#[test]
fn backing_ledger_offsets_match_the_real_percolator_struct() {
    assert_eq!(PERC_HEADER_LEN, 16, "percolator HEADER_LEN");
    assert_eq!(
        OFF_BACKING_MARKET_GROUP,
        PERC_HEADER_LEN + offset_of!(B, market_group),
        "backing-ledger market_group (residual scoping) offset"
    );
    assert_eq!(
        OFF_BACKING_AUTHORITY,
        PERC_HEADER_LEN + offset_of!(B, authority),
        "backing-ledger authority (reward owner) offset"
    );
    assert_eq!(
        OFF_TOTAL_PRINCIPAL,
        PERC_HEADER_LEN + offset_of!(B, total_principal_atoms),
        "backing_capital snapshot offset"
    );
    assert_eq!(
        OFF_TOTAL_EARNINGS,
        PERC_HEADER_LEN + offset_of!(B, total_earnings_atoms),
        "fee-support (capital_at_risk_fee_revenue) snapshot offset"
    );
    assert_eq!(
        OFF_CUMULATIVE_LOSS,
        PERC_HEADER_LEN + offset_of!(B, cumulative_loss_atoms),
        "residual_received snapshot offset"
    );
    // The pinned distribution program id (finding HK) must equal the real distribution program.
    assert_eq!(
        residual_distributor::DISTRIBUTION_PROGRAM_ID,
        distribution_program::id(),
        "pinned distribution program id must match the deployed distribution program"
    );
}

// The subledger Position offsets residual-distributor reads MUST match the subledger's canonical
// layout (finding HF: a wrong owner offset slipped past mocked tests). Cross-pinned to the
// subledger's exported POS_* consts (themselves canaried against Position::serialize there).
#[test]
fn subledger_position_offsets_match_the_real_subledger_layout() {
    use residual_distributor as rd;
    assert_eq!(rd::SUB_POS_POOL, subledger_program::POS_POOL_OFF, "Position.pool offset");
    assert_eq!(rd::SUB_POS_OWNER, subledger_program::POS_OWNER_OFF, "Position.owner offset");
    assert_eq!(rd::SUB_POS_PRINCIPAL, subledger_program::POS_PRINCIPAL_OFF, "Position.principal offset");
    assert_eq!(rd::SUB_POS_WITHDRAWN, subledger_program::POS_WITHDRAWN_OFF, "Position.withdrawn offset");
    assert_eq!(rd::SUB_POS_START_SLOT, subledger_program::POS_START_SLOT_OFF, "Position.start_slot offset");
    assert_eq!(rd::SUB_POS_SHARES, subledger_program::POS_SHARES_OFF, "Position.shares (share-value) offset");
}
