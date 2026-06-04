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

> **Note on layout.** The *non-custodial* multi-program design documented below
> (`genesis-vote/`, `distribution/`, `subledger/`, `twap/` + the deployable
> `twap-program/`, host-side `setup/`) is built and proven **end-to-end against all
> six real binaries** (see [Build & test](#build--test)). The whole lifecycle —
> deposit → vote → distribute → claim → DAO/Squads handoff → permissionless
> uniform-price COIN buy/burn — is exercised across the litesvm `chain.rs` suite.
> The original *custodial* single-program design (`program/`,
> `governance/`) is retained, green, but superseded and removable.

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

**Key model.** The market's asset-0 `asset_admin` (the key-rotator) is the **Squads
vault** throughout — it is the *only* thing that ever rotates the percolator
insurance operator. The **subledger** and **twap** never rotate keys; each only
*consents* (via a powerless `accept_operator` hook) to **receive** the insurance
operator role when Squads grants it. The DAO votes; the DAO, through the 1-week
Squads timelock, issues the key rotation; subledger and twap are pure insurance
fund-managers (top-up / withdraw).

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

- **`subledger`** — the **asset-0 insurance operator during genesis** (granted by
  Squads). Under a **principal-only** policy it mediates deposits (signs the
  Percolator top-up as insurance *authority*) and **owner-authorized, principal-only**
  withdrawals (as insurance *operator*), tracking per-owner attribution
  (`owner, principal, start_slot`). It **never rotates keys** — its `accept_operator`
  only *consents* to receive the operator role Squads grants. The same reusable
  module also backs owner-bound local pools for assets 1..N. The DAO has no authority
  over it beyond the timelock'd Squads grant.
- **`genesis-vote`** — runs the log-time quorum vote, **reading each voter's
  subledger attribution** (principal + hold time) for weight. The winning proposal
  is sealed into the distribution program by CPI. It holds no funds and is not an
  insurance authority.
- **`distribution`** — the fixed COIN pool lives in a vault it controls. A proposal
  is a single on-chain account of up to ~10k `(pubkey, amount)` entries; the sealed
  winner's recipients **claim** their entry permissionlessly; unclaimed is
  **burned**. It never mints.
- **`twap` / `twap-program`** — `twap/` is the library (TWAP schedule, protected
  floor, bid book); `twap-program/` is the deployable BPF program. **After the mint**,
  Squads rotates the asset-0 insurance *operator* from the subledger to the TWAP
  (through the 1-week timelock); the TWAP's own `accept_operator` consents. It then
  runs a permissionless **uniform-price (Dutch) buy/burn auction**: each round it pulls the
  burn-share of market-0 insurance *surplus* (above the principal counter), buys COIN from a
  ranked, uncancellable bid book at one marginal clearing price, and burns it (or sends it to a
  DAO account). It never rotates keys and can never reach principal.
- **The chain** — `DAO → Squads (1/1, 1-week timelock) → {subledger | TWAP} →
  Percolator`. **Squads holds the percolator asset-0 `asset_admin` and is the sole
  key-rotator**: at genesis it grants the insurance operator to the subledger; post-mint
  it rotates it to the TWAP. Every power-expanding authority change is time-locked.

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
   surplus above the principal counter — principal is never touched.
7. **Buy/burn (permissionless, repeating).** The TWAP runs a time-boxed
   **uniform-price (Dutch) auction** each round. Anyone places a bid (escrow COIN, offer
   it for USD at a limit rate; a flat anti-spam fee is burned per bid). A placed bid can't be
   yanked to spoof a pending execute — early it can only be evicted by a strictly-better bid;
   its owner may cancel it (reclaiming the escrowed COIN, the fee stays burned) only after an
   execute has cleared the book once or `2 × round_length` slots pass. When the round's slots expire, anyone
   calls **`execute`**: it pulls the **burn-share** (DAO-set, default 80%) of the current
   surplus as the auction budget, **ratchets the retained share (20%) into the principal
   counter** so it stays in insurance and compounds, clears the whole book at a single
   marginal clearing price (every winner pays the same price; better bidders give less COIN
   and the surplus is refunded), and **burns** the bought COIN — or, if the DAO configures
   it, **sends** it to a treasury account. Winners then **`claim`** their USD. The TWAP
   keeps its USD across rounds and accrues more as surplus refreshes; a DAO **`shutdown`**
   (timelock'd) sweeps the accumulated USD to a supplied address.

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

### 5. Post-handoff: surplus-only, with a ratcheting principal counter
After handoff the insurance authority is the TWAP chain, whose **only** insurance-moving
path is `execute`. Each round it pulls at most the **burn-share** (DAO-set, default 80%) of
`insurance − principal_counter` (the surplus), and **adds the retained share to the principal
counter** — so the protected principal only ever grows, the retained surplus compounds inside
insurance, and the next round's surplus is just the newly-accrued profit. A bare pull that
skips the ratchet cannot exist (the standalone `pull_surplus` was removed). If a market loss
drops insurance below the principal counter, the surplus is zero and the TWAP withdraws nothing
until profits refill it. Principal is never in scope.

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
| `subledger/` | asset-0 **insurance operator** during genesis (principal-only top-up + owner exit, attribution); consents to Squads grants; reusable owner-bound pools for assets 1..N; no DAO authority | built; lib 6 + insurance/percolator 16 + own-vault 5 green |
| `genesis-vote/` | log-time quorum vote (reads subledger attribution); seals the distribution by CPI. Holds no funds | built; lib 3 + seal 5 green |
| `distribution/` | on-chain top-10k `(pubkey,amount)` list; permissionless claim; burn-unclaimed | built; lib 4 + integration 7 green |
| `twap/` | surplus buy/burn *reference* library (pay-as-bid schedule + bid book) — only its overflow-safe rate comparator is reused on-chain; the deployed auction is uniform-price (below) | reference; green |
| `twap-program/` | deployable BPF: the genesis→Squads→TWAP→percolator authority chain **and** the permissionless uniform-price (Dutch) buy/burn auction | built; lib 4 + chain 33 green |
| `setup/` | host-side helper: init the fixed-supply 42M COIN mint (mint + revoke authority) | built; green |
| `program/`, `governance/` | original *custodial* single-program design, superseded but green; removable | green; retained |

### Selected instructions (non-custodial design)
- **subledger:** `init_insurance_pool`, `insurance_deposit` (signs the Percolator
  top-up as insurance *authority* + records `owner, principal, start_slot`),
  `insurance_withdraw` (owner-authorized, principal-only, as insurance *operator*),
  `set_vote_lock` (genesis-vote-gated + owner co-sign), `accept_operator` (powerless
  consent to receive the operator role from Squads — never rotates keys). Plus
  `init_pool` / `deposit` / `withdraw` for the reusable assets-1..N own-vault pools.
  The insurance-pool, gv-config, and twap-config PDAs all commit to their bindings in
  the seed so a permissionless `init` cannot be front-run/squatted (findings P/Q/R).
- **genesis-vote:** `init_config`, `register_proposal` (creator-gated), `vote` (back /
  retract, reading the subledger attribution for weight), `trigger` (seal the winner
  by CPI).
- **distribution:** `init_config`, `create_proposal`, `append_entries` (chunked),
  `seal_winner` (authority-gated = the genesis-vote PDA), `claim` (per-recipient,
  indexed), `burn_unclaimed` (after the window).
- **twap-program:** `init_config`, `accept_operator` / `reconfigure` (burn % 0–100,
  default 80) / `set_reserved_floor` / `set_coin_sink` (burn vs send) / `init_book` /
  `set_reserve` / `shutdown` — all Squads-vault-gated + timelock'd. Plus the
  permissionless **uniform-price (Dutch) buy/burn auction**: `place_bid` (escrow COIN; a
  DAO-set flat fee, default 0.002 COIN, is burned per bid to deter spam — `set_bid_fee`),
  `execute` (the sole insurance puller: pulls the burn-share of surplus, ratchets the
  retained share into the principal counter, clears the whole book at one marginal price,
  burns/sends the COIN), `claim`, and `cancel_bid`. A placed bid can't be yanked to spoof a
  pending execute — early it can only be evicted by a strictly better bid; its owner may
  `cancel_bid` (reclaim the escrowed COIN, fee stays burned) only after an execute has cleared
  the book once or `2 × round_length` slots pass. The surplus floor (finding O) + correct
  insurance slab offset (finding T) live in `execute`.

---

## The authority chain & 1-week timelock

`DAO → Squads (1/1, 1-week timelock) → TWAP → Percolator`.

- The genesis market's keys are held by a program-created [Squads
  v4](https://squads.so) 1/1 multisig with a **one-week** timelock.
- Squads holds the percolator **asset-0 `asset_admin`** and is the **sole
  key-rotator**. At genesis it grants the insurance authority+operator to the
  **subledger pool** (which consents via `accept_operator`); post-mint it rotates the
  operator to the **TWAP PDA** — both via `UpdateAssetAuthority{asset_index:0}`.
  percolator requires the incoming key to co-sign, so the subledger/twap each expose a
  powerless `accept_operator` consent hook; neither can rotate keys itself.
- Every authority rotation that could expand power over user funds passes through
  the **one-week timelock**, which is the user-exit backstop (Safety §3). The
  builders for this chain live in `twap/` (`percolator_v16`, `surplus`).

---

## Build & test

```bash
# build the deployable BPF programs (each self-contained)
cargo build-sbf --manifest-path subledger/Cargo.toml
cargo build-sbf --manifest-path distribution/Cargo.toml
cargo build-sbf --manifest-path genesis-vote/Cargo.toml
cargo build-sbf --manifest-path twap-program/Cargo.toml

# tests (RUST_MIN_STACK is needed for the deep nested-CPI e2e)
RUST_MIN_STACK=8388608 cargo test --manifest-path subledger/Cargo.toml
RUST_MIN_STACK=8388608 cargo test --manifest-path genesis-vote/Cargo.toml
cargo test --manifest-path distribution/Cargo.toml
RUST_MIN_STACK=8388608 cargo test --manifest-path twap-program/Cargo.toml

# the whole lifecycle across ALL SIX real binaries in one litesvm instance —
#   deposit -> vote -> distribute -> claim -> DAO/Squads handoff -> buy/burn auction
#   (the winner sells COIN back into the surplus buy/burn; it is really burned):
RUST_MIN_STACK=8388608 cargo test --manifest-path twap-program/Cargo.toml \
    --test chain e2e_full_genesis_to_buy_burn
```

Tests load the **real** binaries (Percolator at
`../percolator-prog/target/deploy/percolator_prog.so`, real Squads v4 at
`program/tests/fixtures/squads_v4.so`, plus the locally-built subledger / genesis-vote
/ distribution / twap-program) — CPIs are exercised against the actual programs, not
mocks. Running the e2e needs the subledger/genesis-vote/distribution/twap-program
`.so` files prebuilt (`cargo build-sbf` above) and `../percolator-prog` built.

## License

Licensed under the [Apache License 2.0](LICENSE). Provided "as is", educational use
only — see the disclaimer above.
