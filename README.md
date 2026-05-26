# Percolator Meta

MetaDAO-governed Solana programs for bootstrapping Percolator markets, distributing the initial COIN supply, and accounting for insurance/backing capital without giving the DAO custody over depositor principal.

## Genesis Plan

1. MetaDAO initializes `CoinConfig` with a configurable bootstrap delay in slots. A zero delay is live immediately; the intended launch setting can be a six-month slot delay.
2. Futarchy creates `GenesisConfig` with a fixed reward supply and a deposit window. The default window is about one week at 400ms slots and is capped by the remaining bootstrap delay.
3. During that short window, users deposit base units into `genesis_vault`. One deposited base unit equals one genesis vote unit.
4. Genesis depositors take the first market's code and market risk in exchange for voting power over the fixed genesis COIN distribution.
5. Futarchy kickstarts the first Percolator market with the pooled base units as a 50/50 split: `floor(total / 2)` to insurance and the remainder to backing. Kickstart also configures the engine with a capital-protected insurance withdraw policy (`max_bps=10000, deposits_only=1, cooldown=0`) so deposited principal — never market profits — can be drawn back out.
6. At any time before voting starts (the whole pre-live bootstrap phase), a genesis depositor may exit via `genesis_bootstrap_withdraw`: they forfeit all vote units and recover principal. Before kickstart the refund comes straight from `genesis_vault`; after kickstart the caller pulls their principal back from the market's insurance fund and backing bucket (pro-rata against what the market can currently cover, with any shortfall left as a later claim).
7. Before finalization, futarchy may recover bootstrap market insurance/backing principal and earnings only back into `genesis_vault`.
8. After the bootstrap delay, depositors vote on allocation items. Futarchy can mint COIN only for majority-approved items, and 100% of the fixed reward supply must be minted before finalization.
9. Finalization requires both a kicked bootstrap market and `minted_supply == reward_supply`.
10. After finalization, depositors can withdraw up to their original base-unit deposit. If the market lost capital, withdrawals are pro-rata against recovered funds and unpaid principal remains reserved for later recovery.

Any surplus in `genesis_vault` above outstanding genesis principal is drawable by futarchy after finalization.

## Post-Genesis Lifecycle

After `activate_live`, anyone may create additional Percolator markets through `init_percolator_market`. The caller funds the market account, but the COIN-specific `percolator_market_admin` PDA becomes Percolator admin.

Futarchy controls the market lifecycle through explicit meta-program instructions:

- Percolator market init, asset lifecycle, oracle setup, fee policy, resolve, and close-slab cleanup.
- Insurance and backing risk-vault setup and reward/fee policy.
- Builder-code approvals by `(coin_mint, builder_program, code_hash)` plus a terms hash.

Raw Percolator `UpdateAuthority` and funding/withdrawal tags are not exposed through the generic admin proxy. Custody-bearing authority changes must use explicit setup paths.

Cranks and permissionless Percolator maintenance remain external.

## Capital Accounting

External insurance/backing depositors use risk vaults:

- Deposits and principal withdrawals are tracked per depositor in this program.
- Percolator supplies aggregate counters and accumulators.
- Backing earnings can be claimed by depositors minus a futarchy-configured DAO fee routed to the main insurance vault.
- Insurance/backing lockups and delayed withdrawals are enforced by the meta program.

Genesis depositors are different: their principal is at risk in the bootstrap market, and their vote units become worthless after finalization/withdrawal. They are not locked in, though — until voting starts they may exit at any time (`genesis_bootstrap_withdraw`), forfeiting their vote and reclaiming principal from the vault or, post-kickstart, from the market's insurance fund and backing bucket.

## Tested Surface

The LiteSVM suite runs against the real percolator, governance, rewards, and Squads v4 binaries. A single end-to-end test (`test_full_genesis_to_dao_lifecycle_end_to_end`) chains the entire lifecycle with no shortcuts: deposit → real market init → Squads 1/1+48h creation → real 50/50 kickstart → mid-bootstrap depositor exit pulling principal back from the live market → go live → propose/vote/mint 100% of supply → recover market principal → finalize → hand the Squads config-authority to the winning DAO → remaining depositors withdraw. Individual phases are also covered in isolation:

- Configurable bootstrap delay, short genesis deposit window, and live activation.
- Genesis deposit, vote, 100% supply mint, finalize, withdrawal, surplus, recovery, and 50/50 kickstart.
- Pre-voting bootstrap exit: full refund + vote forfeit before kickstart, and post-kickstart principal pull from the insurance fund + backing bucket; exit closes once voting starts.
- Permissionless market creation plus futarchy-controlled Percolator lifecycle/admin operations.
- Insurance/backing risk-vault setup, sync, depositor withdrawal, and backing earnings fee routing.
- Builder approvals and executable-program validation.
- Genesis→DAO Squads handover: program-created 1/1 + 48h multisig and config-authority rotation, driven through governance against the real mainnet Squads binary, plus a standalone harness proving timelock enforcement and upgrade-key rotation (`program/tests/squads_handover.rs`).
- Disabled legacy staking/reward-pool instruction tags.

Current full-suite smoke target:

```bash
cargo build-sbf --manifest-path governance/Cargo.toml
cargo build-sbf --manifest-path program/Cargo.toml
RUST_MIN_STACK=8388608 cargo test --manifest-path program/Cargo.toml --test integration
```

The integration test also requires a built Percolator BPF binary at `../percolator-prog/target/deploy/percolator_prog.so`.

## Instructions

| Tag | Instruction | Purpose |
|-----|-------------|---------|
| 3 | `init_coin_config` | One-time COIN governance/mint setup |
| 8 | `mint_reward` | Governance-gated discretionary COIN mint |
| 10 | `transfer_mint_authority` | Transfer or burn COIN mint authority |
| 11 | `activate_live` | Move from bootstrap to live after delay |
| 12 | `init_risk_vault` | Set up insurance/backing depositor accounting |
| 13 | `register_risk_vault_authority` | Register risk-vault PDA with Percolator |
| 14 | `risk_deposit` | External insurance/backing principal deposit |
| 15 | `risk_request_withdraw` | Request delayed principal withdrawal |
| 16 | `risk_withdraw` | Withdraw matured principal |
| 17 | `sync_risk_vault` | Sync Percolator aggregate counters |
| 18 | `risk_claim_rewards` | Claim backing earnings minus DAO fee |
| 19 | `init_percolator_market` | Permissionless Percolator `InitMarket` via PDA admin |
| 20 | `percolator_admin` | Futarchy-gated Percolator lifecycle/admin CPI |
| 21 | `init_genesis_bootstrap` | Create genesis config, deposit window, and base-token vault |
| 22 | `genesis_deposit` | Bootstrap base-unit deposit, 1 unit = 1 vote |
| 23 | `genesis_withdraw` | Post-finalization principal withdrawal |
| 24 | `genesis_mint_reward` | Mint approved genesis allocation |
| 25 | `finalize_genesis` | Complete genesis after kickstart and full mint |
| 26 | `draw_genesis_surplus` | Draw surplus above outstanding principal |
| 27 | `kickstart_genesis_market` | Deploy genesis principal 50/50 to first market |
| 28 | `recover_genesis_market` | Recover bootstrap market funds to `genesis_vault` |
| 29 | `init_genesis_distribution` | Create a genesis allocation item |
| 30 | `vote_genesis_distribution` | Vote on a genesis allocation item |
| 31 | `approve_builder` | Governed builder-code and terms registry |
| 32 | `init_genesis_squads` | Futarchy-gated CPI creating the per-coin Squads 1/1 multisig (48h timelock) |
| 33 | `handover_genesis_squads` | Rotate the multisig `config_authority` to the winning DAO after finalization |
| 34 | `genesis_bootstrap_withdraw` | Permissionless pre-voting exit: refund principal and forfeit vote during bootstrap |

Tags `0`, `1`, `2`, `4`, `5`, `6`, `7`, and `9` are intentionally disabled legacy slots.

## Squads Handover

The genesis market's governance is held by a program-created [Squads v4](https://squads.so) multisig: a controlled 1/1 multisig with a 48-hour timelock whose `config_authority` is this program's `percolator_market_admin` PDA from genesis (`init_genesis_squads`). The multisig address is deterministic per coin (create-key seed `[b"genesis_squads", coin_mint]`).

Control transfers to the winning genesis DAO by rotating that `config_authority` (`handover_genesis_squads`, permitted only once genesis is finalized). Percolator's own `UpdateAuthority` is never touched, so no incoming-authority consent is required and depositor custody is never re-pointed. Both instructions are reached through the governance adapter (tags `17`/`18`).

## Key PDAs

| Account | Seeds |
|---------|-------|
| `CoinConfig` | `[b"coin_cfg", coin_mint]` |
| `CoinMintAuthority` | `[b"coin_mint_authority", coin_mint]` |
| `percolator_market_admin` | `[b"percolator_market_admin", coin_mint]` |
| `GenesisConfig` | `[b"genesis_cfg", coin_mint]` |
| `GenesisVault` | `[b"genesis_vault", coin_mint]` |
| `GenesisPosition` | `[b"genesis_position", genesis_cfg, user]` |
| `GenesisDistribution` | `[b"genesis_distribution", genesis_cfg, proposal_id]` |
| `GenesisDistributionVote` | `[b"genesis_distribution_vote", distribution, voter]` |
| `RiskVaultCfg` | `[b"risk_vault", market_slab, kind_domain]` |
| `RiskTokenVault` | `[b"risk_token_vault", market_slab, kind_domain]` |
| `RiskPosition` | `[b"risk_position", risk_vault, user]` |
| `RiskLedger` | `[b"risk_ledger", market_slab, kind_domain]` |
| `BuilderApproval` | `[b"builder_approval", coin_mint, builder_program, code_hash]` |
| Squads create-key | `[b"genesis_squads", coin_mint]` (this program) |
| Squads multisig | `[b"multisig", b"multisig", create_key]` (Squads v4) |
