# Percolator Meta

A **non-custodial, Sybil-resistant governance bootstrap** for Percolator markets.
Insurance depositors vote on how a fixed COIN supply is distributed; the winning
distribution becomes the MetaDAO. The design is deliberately split into small,
independently-audited programs, and **no program in this repo is ever in the
user-fund custody or withdrawal path** beyond a tightly-constrained, time-locked
authority.

## ⚠️ Status & Disclaimer

Experimental, **educational-use-only** software, provided **AS IS** with no
warranties or conditions of any kind (see [LICENSE](LICENSE)). Not financial
advice and not a guarantee of correctness or fitness for any purpose. Participants
put real capital **at risk** in a live market and can lose it to market losses —
the deposit is a Sybil-resistance bond, not an investment. Use at your own risk.

> **Note on layout.** This repo is mid-migration from a single *custodial* genesis
> program (`program/`, `governance/` — the original design, still building/green)
> to the *non-custodial* multi-program design documented below
> (`genesis-vote/`, `distribution/`, `subledger/`, `twap/`). The sections below
> describe the **target non-custodial design and its safety boundaries**; build
> status per piece is noted under [Programs](#programs).

---

## Premise

Depositing is a **Sybil check, not an investment.** Capital is put at risk in
Percolator market-0 insurance for one reason — to earn time-weighted voting power
over the COIN distribution. There is **no yield and no profit share**. The cost of
a vote is the capital-at-risk itself, which is what makes votes expensive to Sybil.

The COIN is a **fixed, pre-existing supply with no mint authority.** Genesis does
not mint; it *allocates* the fixed pool. The winning distribution's COIN **is** the
MetaDAO token, and control of the market keys transfers to it through a
time-locked Squads handover.

---

## Architecture

```
   depositor                                        anyone (proposer)
      │ deposit                                            │
      ▼                                                    ▼
 ┌─────────────┐   attribution   ┌──────────────┐  seal  ┌──────────────────────┐
 │  subledger  │ ──(read)──────▶ │ genesis-vote │─(CPI)─▶│     distribution     │
 │  = asset-0  │                 │ log-time vote│        │ (pubkey,amount) list  │
 │  insurance  │                 │   + quorum   │        │  → claim → burn       │
 │  authority  │                 └──────────────┘        └──────────────────────┘
 └──────┬──────┘  top-up / principal-only exit              fixed COIN pool (vault)
        │ (signs as authority)
        ▼
 ┌─────────────┐   surplus (>floor)    ┌───────────┐
 │ Percolator  │ ────────────────────▶ │   twap    │  buy / burn COIN
 │ m-0 insur.  │                       │ buy/burn  │
 └─────────────┘                       └───────────┘
        ▲ post-mint: insurance authority rotates  subledger ──▶ twap
        │            through the 1-week timelock
        └────────  DAO → Squads (1/1, 1-week timelock) → Percolator

   subledger/ also serves reusable owner-bound pools for assets 1..N (no DAO authority)
```

- **`subledger`** — the **asset-0 insurance authority during genesis**. Market-0
  insurance is configured with the subledger program as authority under a
  **principal-only** withdrawal policy: it mediates deposits (signs the Percolator
  top-up) and **owner-authorized, principal-only** withdrawals, tracking per-owner
  attribution (`owner, principal, start_slot`). The same reusable module also backs
  owner-bound local pools for assets 1..N. The DAO has no authority over it.
- **`genesis-vote`** — runs the log-time quorum vote, **reading each voter's
  subledger attribution** (principal + hold time) for weight. The winning proposal
  is sealed into the distribution program by CPI. It holds no funds and is not the
  insurance authority.
- **`distribution`** — the fixed COIN pool lives in a vault it controls. A proposal
  is a single on-chain account of up to ~10k `(pubkey, amount)` entries; the sealed
  winner's recipients **claim** their entry permissionlessly; unclaimed is
  **burned**. It never mints.
- **`twap`** — **after the mint**, the insurance authority **rotates from the
  subledger to the TWAP** (through the 1-week Squads timelock). The TWAP then pulls
  market-0 insurance *surplus* (above a protected floor), fills ranked
  COIN-for-USDC bids, and burns the COIN. It can never reach principal.
- **The chain** — `DAO → Squads (1/1, 1-week timelock) → TWAP → Percolator`. Squads
  holds the percolator asset-0 `asset_admin`; post-mint it installs the TWAP PDA as
  asset-0 insurance operator; every power-expanding authority change is time-locked.

---

## Lifecycle

1. **Deposit (through the subledger = insurance authority).** A depositor deposits
   into market-0 insurance via the **subledger** program. The subledger is the
   asset-0 insurance authority, so it signs the Percolator `TopUpInsurance` (real
   Percolator insurance is authority-gated, not permissionless) and records
   per-owner attribution (`owner, principal, start_slot`). The capital lives in
   Percolator; the subledger holds no separate custody and its only powers are
   top-up and principal-only owner exit. `start_slot` is last-write-time, so topping
   up resets the vote clock.
2. **Vote (log-time, quorum).** `genesis-vote` reads the voter's subledger
   attribution. One voter, one proposal. Weight = `floor(log2(hold_time)) ×
   principal`, resolved at vote time. Backing a different proposal requires
   retracting first. Quorum = `total_voted_principal × 2 > outstanding`; winner =
   `support_weight × 2 > total_cast_weight`.
3. **Exit (any time, principal-only, owner-authorized).** A non-voter exits freely;
   a voter retracts first. Exit goes through the **subledger**: an owner-authorized,
   principal-only `WithdrawInsuranceLimited` (the `deposits_only` policy caps it at
   deposited principal, never profits). Exiting shrinks `outstanding`, so quorum
   recomputes against whoever stays — *those who stay decide*.
4. **Trigger (permissionless).** The first proposal to clear quorum + a weighted
   majority is sealed via CPI into the distribution program (the genesis-vote
   config PDA is the distribution's seal authority). No mint.
5. **Claim / burn.** The winning distribution's recipients claim their `(pubkey,
   amount)` entry from the fixed COIN vault; anything unclaimed when the window
   closes is burned.
6. **Handoff (post-mint).** Control rotates `DAO → Squads (1-week) → TWAP/Percolator`.
   The asset-0 insurance authority moves from the constrained **subledger**
   (principal-only) to the surplus-only **TWAP**. Post-handoff, the TWAP buys/burns
   surplus above the protected floor — principal is never touched.

---

## Safety boundaries

The core guarantee is **the DAO (or a bug in any genesis program) cannot take a
user's principal.** It rests on layered, independent boundaries:

### 1. Non-custodial — nothing is wrapped
User capital lives in **Percolator insurance** (or the owner-bound `subledger`
pools for assets 1..N), never in a genesis-owned vault the DAO can sweep. The
genesis programs do **attribution and reward accounting only**. A bug in
`genesis-vote` can at worst *misweight a vote*; a bug in `distribution` can at
worst *misallocate the fixed COIN pool* — neither can move user capital.

### 2. The insurance authority is constrained
During genesis the asset-0 insurance authority is the **subledger** program under a
principal-only policy. Its power is limited to exactly two things:
- **add** insurance (`TopUpInsurance`), and
- **owner-authorized, principal-only exit** (`WithdrawInsuranceLimited` under a
  `deposits_only=1, max_bps=10000` policy, additionally capped to the *caller's own*
  recorded principal).

It can never withdraw to itself, never take another user's principal, and never
touch market profits.

### 3. The 1-week Squads timelock is the backstop
For the DAO to gain *un*constrained power over insurance, it must **rotate the
asset-0 authority** away from the constrained subledger (e.g. to the TWAP at
handoff, or anywhere else) — and that rotation runs `DAO → Squads (1-week timelock)
→ Percolator UpdateAssetAuthority`. The dangerous change is **delayed a full week,
in the clear**, with the old constrained authority still live the entire time.
Users observe the pending rotation and **exit their principal during the window**,
before any new authority is effective.

This is the robust layer: it bounds the blast radius of *any* bug in the
genesis-vote / distribution / chain code to "users get a one-week, pre-announced
exit window." The one hard requirement is that **the exit stays available while a
rotation is pending** (it does — the old authority is unchanged until the timelock
elapses).

### 4. Fixed supply, no mint authority
The COIN mint has **no mint authority**. The fixed supply is held by the
distribution vault and distributed by claim; unclaimed is burned. No program can
mint COIN, so there is no inflation/dilution vector and no "mint to drain" path.

### 5. Post-handoff: surplus-only
After handoff the insurance authority is the TWAP chain, which can only pull
**surplus above a protected floor** (`reserved_principal + retained_surplus_floor`)
to buy/burn COIN. If a market loss drops insurance below the floor, the TWAP
withdraws nothing until profits refill it. Principal is never in scope.

### The money map (where funds are and how they move)
| Funds | Custody | In | Out |
|---|---|---|---|
| Insurance principal | Percolator market-0 | subledger top-up (authority-signed) | owner-authorized principal-only exit (subledger); never below floor post-handoff |
| Fixed COIN pool | distribution vault | one-time, pre-existing supply | recipient claim; unclaimed burned |
| Surplus (>floor) | Percolator market-0 | market profit | TWAP buy/burn of COIN |
| Subledger pools (1..N) | per-asset, owner-bound | user deposit | owner-only exit (no DAO authority) |

---

## Programs

| Crate | Role | Status |
|---|---|---|
| `subledger/` | asset-0 **insurance authority** during genesis (principal-only top-up + owner exit, attribution); also reusable owner-bound pools for assets 1..N; no DAO authority | built; 10 tests green. Percolator insurance-authority wiring + `start_slot` for the vote in progress |
| `genesis-vote/` | log-time quorum vote (reads subledger attribution); seals the distribution by CPI. Holds no funds; not the insurance authority | built; unit + cross-program seal tests green |
| `distribution/` | on-chain top-10k `(pubkey,amount)` list; permissionless claim; burn-unclaimed | built; 7 tests green |
| `twap/` | surplus buy/burn (TWAP schedule, protected floor, ranked bid book, partial fills) + percolator/squads authority-chain CPI builders | faithful library port; 24 tests green. Deployable BPF wrapper + chain e2e in progress |
| `program/`, `governance/` | original *custodial* single-program design being superseded | green; retained until the non-custodial path is fully proven |

### Selected instructions (non-custodial design)
- **subledger** (the insurance authority): `init_pool`, `deposit` (signs the
  Percolator insurance top-up + records `owner, principal, start_slot`), `withdraw`
  (owner-authorized, principal-only). The same module serves the reusable assets-1..N
  pools.
- **genesis-vote:** `init_config`, `register_proposal`, `vote` (back / retract,
  reading the subledger attribution for weight), `trigger` (seal the winner by CPI).
- **distribution:** `init_config`, `create_proposal`, `append_entries` (chunked),
  `seal_winner` (authority-gated = the genesis-vote PDA), `claim` (per-recipient,
  indexed), `burn_unclaimed` (after the window).

---

## The authority chain & 1-week timelock

`DAO → Squads (1/1, 1-week timelock) → TWAP → Percolator`.

- The genesis market's keys are held by a program-created [Squads
  v4](https://squads.so) 1/1 multisig with a **one-week** timelock.
- Squads holds the percolator **asset-0 `asset_admin`**, and installs/rotates the
  **TWAP PDA as asset-0 `INSURANCE_OPERATOR`** via percolator
  `UpdateAssetAuthority{asset_index:0, kind:INSURANCE_OPERATOR}`.
- Every authority rotation that could expand power over user funds passes through
  the **one-week timelock**, which is the user-exit backstop (Safety §3). The
  builders for this chain live in `twap/` (`percolator_v16`, `surplus`).

---

## Build & test

```bash
# the non-custodial programs (each self-contained)
cargo build-sbf --manifest-path subledger/Cargo.toml
cargo build-sbf --manifest-path distribution/Cargo.toml
cargo build-sbf --manifest-path genesis-vote/Cargo.toml
cargo test -p subledger-program -p distribution-program -p genesis-vote-program
cargo test -p twap

# the original custodial program (real percolator/governance/squads binaries)
cargo build-sbf --manifest-path governance/Cargo.toml
cargo build-sbf --manifest-path program/Cargo.toml
RUST_MIN_STACK=8388608 cargo test --manifest-path program/Cargo.toml --test integration
```

Integration tests load the **real** binaries (Percolator at
`../percolator-prog/target/deploy/percolator_prog.so`, real Squads v4) — CPIs are
exercised against the actual programs, not mocks.

## License

Licensed under the [Apache License 2.0](LICENSE). Provided "as is", educational use
only — see the disclaimer above.
