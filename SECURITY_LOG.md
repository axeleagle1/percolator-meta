# Security analysis log (adversarial LOF/DOS sweep)

Running note so the 5-min loop doesn't repeat vectors. Format: vector → verdict.

## Analyzed

### [DESIGN] U. Buy/burn uniform-price (Dutch) auction — invariants (twap-program)
The COIN buy/burn settlement is a permissionless, time-boxed uniform-price auction (twap-program
tags 5-11). Security properties, each pinned by a chain.rs e2e against the real binaries:
- **Anti-spoofing**: a placed bid CANNOT be cancelled — there is no withdraw instruction; the only
  way a bid leaves the book before `execute` is eviction by a STRICTLY better bid (which refunds the
  evictee). A bidder cannot yank a bid right before the auction runs, nor stack a second bid. This is
  the deliberate fix vs the `twap/` library's `withdraw_bid`/`close_bid_escrow` (left UNUSED on-chain).
  Pinned: `e2e_bid_cannot_be_cancelled_only_evicted_by_a_better_bid`.
- **Uniform marginal clearing**: bids ranked by COIN-per-USD (overflow-safe continued-fraction
  comparator), filled best-first until the budget is spent; EVERY filled bid clears at the marginal
  (lowest-accepted) rate P*, so better bidders give less COIN than offered (surplus refunded). A
  DAO-set **reserve rate** caps the price the protocol will pay, bounding the marginal-bid
  manipulation where a whale's huge expensive bid drags P* down. Pinned:
  `e2e_buy_burn_uniform_price_dutch_auction` (asserts the real COIN mint supply drops by the bought
  amount — an actual burn — and both winners pay the SAME P*).
- **`execute` is the SOLE insurance puller**: the standalone `pull_surplus` was removed. `execute`
  (permissionless, gated on round expiry) pulls only `surplus * buy_burn_bps` (default 80%) as the
  budget and **ratchets the retained share into the principal counter** (`reserved_floor +=`), so the
  retained 20% stays in insurance, is reclassified as protected principal, and compounds — a bare
  pull that skips the ratchet can no longer exist. Finding O's floor (slab read at offset 749, finding
  T) lives here now. Pinned: `e2e_execute_pulls_only_burn_share_and_ratchets_principal`.
- **Permissionless place/execute/claim**; **futarchy-configurable COIN sink** (burn default OR send to
  an account, Squads-gated); **DAO shutdown** sweeps the TWAP's accumulated USD to a supplied address
  (Squads-gated only). Pinned: `e2e_shutdown_sweeps_holding_only_via_squads`.
- COIN escrow is pooled in ONE book-escrow account so `execute` burns/pays in O(1) CPIs regardless of
  bid count; the book is a fixed 32-slot array with O(n) worst-bid eviction.

### [FIXED] T. Insurance slab offset read `vault`, not `insurance` (subledger + twap) — finding-O class LOF
Both the subledger pro-rata haircut (`subledger/src/lib.rs PERC_INSURANCE_OFFSET`) and the twap
surplus pull (`twap-program/src/lib.rs INSURANCE_OFFSET`) read the asset-0 insurance fund straight
from the percolator market slab at a hardcoded byte offset. Both used `448 + 285`, derived from a
hand-counted struct layout that assumed `V16ConfigAccount` was 233 bytes. It is actually **249**,
so `+285` is the `vault` field; `insurance` is at **`448 + 301`** (slab offset 749, confirmed by
`core::mem::offset_of!(MarketGroupV16HeaderAccount, insurance)` against the real percolator crate,
and by scanning a live slab: vault@733, insurance@749, remaining_budget_total@893).
`vault` = total tokens in the market = `insurance + trader capital + pnl`, so it is `>= insurance`
and equal only when there is NO trading capital (exactly the case in every prior test, which is why
the old `insurance_offset_matches_real_percolator_slab` canary — funding via TopUp, which bumps BOTH
vault and insurance equally — could not tell them apart and the bug shipped).
Impact (both the finding-O failure class): with live trader capital in the market,
(a) the twap `pull_surplus` would treat trader/depositor capital as withdrawable "surplus" above the
reserved floor, and (b) the subledger pro-rata haircut would over-count the fund and under-charge the
haircut (paying early exiters too much, stranding late ones). FIX: both constants → `448 + 301`.
Pinned now by: (1) twap canary asserting `INSURANCE_OFFSET == 448 + offset_of!(.., insurance)` AND
bumping the adjacent `vault` field to a distinct sentinel to prove the read returns `insurance` not
`vault`; (2) subledger `impair_market` helper deriving every loss-coupled offset from the real struct
via `offset_of!`/value canaries. Tests: subledger `impaired_insurance_exit_is_pro_rata` (order-
independent 50% haircut against the real binary) + full twap chain (33) green.

### [FIXED] A. Stale quorum denominator → minority capture (genesis-vote trigger)
`trigger` checked quorum against `config.outstanding_principal`, a CACHE synced
only on `vote`. Attack: vote early when the subledger pool is tiny (cache=6), let
honest deposits grow the pool to 1006 without a re-vote, then trigger: 6*2 > 6
(stale cache) "passes" → a 6-principal minority captures the whole COIN
distribution. FIX: trigger now takes the subledger pool account and re-reads the
LIVE `outstanding_principal` for the quorum check. Regression:
`genesis-vote/tests/seal.rs::trigger_uses_live_pool_outstanding_not_stale_cache`.

### [FIXED] B. Vote outlives capital (genesis-vote support tallies are snapshots)
FIXED via a cross-program vote-lock (a boolean, simpler than the candidate
`locked_principal` since one position = one whole vote). The subledger Position
gained a `vote_locked` flag and the Pool a `vote_authority` (= the genesis-vote
config PDA); a new `IX_SET_VOTE_LOCK` (tag 6) lets ONLY that authority toggle it,
and `process_insurance_withdraw` refuses while set. genesis-vote `vote` now CPIs
SetVoteLock(1) on back / SetVoteLock(0) on retract (config PDA signs). A live
ballot pins its principal in the subledger; capital can leave only after the vote
is retracted — which the owner always controls, so no permanent-freeze risk. This
restores the "voters must retract before exit" invariant the single-program
genesis had. Regression (KEPT, real-percolator e2e):
insurance_percolator.rs::vote_locked_principal_cannot_exit_until_retracted
(vote → withdraw refused → retract → withdraw succeeds). Original analysis:

genesis-vote records `voted_principal`/`support_*` as a snapshot at vote time, but
the capital lives in the SUBLEDGER and the subledger `insurance_withdraw` does NOT
require the genesis-vote ballot to be retracted first. So a voter can vote
(support += P), then withdraw P from the subledger (capital returned), and the
ballot still counts toward quorum/majority with ZERO capital at risk — a free /
Sybil vote. WORSE after fix A: the live-outstanding denominator shrinks on exit
while the snapshot numerator stays, inflating quorum. The old single-program
genesis enforced retract-before-exit; the cross-program split broke it.
Candidate fix: genesis-vote vote/retract CPIs the subledger to set a
`locked_principal` on the position; subledger `insurance_withdraw` cannot reduce
principal below `locked_principal`. (Subledger exposes the lock to a registered
vote-authority only.) NEXT ITERATION.

### [FIXED] B2. Vote-lock freeze-after-finalize (regression from B's fix)
Adversarial self-review of B's fix: genesis-vote `vote` rejected ALL actions once
a proposal was sealed (`pv.executed`), so a WINNING voter could never retract to
clear the subledger vote-lock → principal frozen forever. FIX: post-seal forbid
only NEW backing (VOTE_BACK); always allow VOTE_RETRACT (clears the lock; seal is
immutable; post-seal tally writes are unread). Regression:
insurance_percolator.rs::winning_voter_can_retract_and_exit_after_finalize
(drives the real trigger to seal, then proves retract+exit works post-finalize).

### [FIXED] E. Distribution vault solvency not enforced at init (claim-race LOF)
`init_config` recorded `total_supply` from instruction data and validated the
vault's mint/owner, but never checked the vault was FUNDED to total_supply. The
seal only enforces `total_amount <= total_supply` (the claimed number), so a config
whose vault held less than promised would let early claimants drain it and STRAND
honest late claimants (first-come claim race). FIX: init now requires
`vault.amount >= total_supply` (InsufficientFunds otherwise), tying the promised
supply to real tokens — a config can never promise more than the vault holds, and
since seal caps total_amount at total_supply, every sealed proposal is fully
claimable. Test (KEPT): distribution.rs::init_config_rejects_an_underfunded_vault.

### [BLOCKED] Distribution claim/seal/append/burn — full adversarial read (tick 5)
Probed the entire distribution fund-exit surface; all well-defended:
- claim: pull model (`pk == recipient.key`), sealed-proposal pin, vault pin, window
  bound, index bound, double-claim zeroing (amount==0 reject). recipient_ata is
  unchecked but that is the recipient directing their OWN allocation — not theft.
- append_entries: rejected once `header.sealed || config.is_sealed()` (no post-seal
  drain), running `total_amount <= total_supply`, checked_add, capacity bound.
- seal_winner: authority==config.authority, not-already-sealed, `header.config ==
  config` (no foreign-proposal seal), `entry_count > 0`, total_amount <= supply.
- burn_unclaimed: sealed-only, vault+mint pinned, `clock.slot < window_end` blocks
  premature burns. No tests added (would only re-assert existing checks).

### [FIXED] F. Foreign distribution proposal → unsealable winner (finalize DOS)
genesis-vote `register_proposal` checked `distribution_proposal.owner ==
config.distribution_program` but NOT that the proposal belonged to
`config.distribution_config`. A proposal owned by the distribution program but
under a DIFFERENT config could be registered and voted on; `trigger` pins the
distribution_config to `config.distribution_config` and CPIs SealWinner against it,
which the distribution rejects on `header.config` mismatch. So a foreign-linked
proposal that WON could never be sealed → the genesis could never finalize (the
whole bootstrap bricks; the COIN/MetaDAO never forms). Voters can still retract+exit
(no fund freeze) but the protocol is dead. FIX: register_proposal now reads the
distribution proposal header (disc `DISTPRP1`, config at [8..40]) and requires it to
equal `config.distribution_config` — every votable proposal is guaranteed sealable.
Test (KEPT, real cross-program e2e with a second distribution config):
insurance_percolator.rs::register_rejects_foreign_distribution_proposal.
NOTE (low-risk, NOT fixed): a proposal under the correct config but with
entry_count==0 would also fail seal (seal requires entry_count>0); but entries can
be appended any time pre-seal and no rational voter backs an empty (zero-allocation)
proposal, so it is self-correcting, not a forced DOS.

### [FIXED] G. Hostile vote_authority → griefing freeze of depositors (LOF)
`init_insurance_pool` is permissionless and records `vote_authority` as-is (no
validation). An attacker could front-run creation of the genesis's (COIN, asset-0)
insurance pool with `vote_authority = attacker`. `set_vote_lock` required ONLY the
vote_authority to sign — so the attacker could then lock ANY depositor's position
(`set_vote_lock(victim, 1)`), and `insurance_withdraw` refuses while locked. The
real genesis-vote config PDA is NOT the authority, so it cannot unlock → victims'
principal FROZEN forever (griefing LOF). FIX: `set_vote_lock` now also requires the
position OWNER to sign (and `position.owner == owner`). A position can therefore
only be (un)locked in the context of its owner acting on their own vote — the only
legitimate case. The vote_authority gate stays so the owner cannot self-unlock and
bypass retract (which would re-open finding B). genesis-vote `vote` propagates the
voter's signature into the SetVoteLock CPI. Test (KEPT, real-percolator e2e):
insurance_percolator.rs::hostile_vote_authority_cannot_freeze_a_depositor.

### [FIXED] H. genesis-vote init_config didn't bind its wired dependencies
`init_config` stored `distribution_config` and `subledger_pool` as-is, with NO
check that they bind back to the config being created. An honest orchestrator could
thus wire the genesis to a poisoned/foreign pool or distribution config (e.g. the
front-run pool of finding G), silently bricking it: a subledger pool whose
vote_authority != this config PDA makes every vote's SetVoteLock CPI fail (no one
can vote), and a distribution config whose seal authority != this config PDA (or
for another mint) makes trigger's SealWinner always fail (finalize DOS). FIX: init
now requires, for this coin_mint, that (a) distribution_config is a real DISTCFG1
owned by distribution_program with coin_mint match and authority == config PDA, and
(b) subledger_pool is a real SUBPOOL1 owned by subledger_program with mint match and
vote_authority == config PDA. The config can only be created against dependencies
that recognize it — fail-fast at init instead of a mysterious brick at vote/trigger.
Complements finding G (G stops the freeze; H stops building on a poisoned pool).
Test (KEPT): seal.rs::init_config_rejects_pool_not_bound_to_this_config.

### [BLOCKED] Own-vault init_pool vault-substitution theft (now pinned by a test)
Probed: own-vault `deposit` (tag 1) transfers owner -> pool.vault and `withdraw`
(tag 2) pays from pool.vault signed by the pool PDA. If `init_pool` accepted a vault
owned by the attacker, they could stand up a pool, lure a deposit into their own
token account, and drain it directly via SPL (program withdraw would fail). Already
blocked: init_pool pins `vault_state.owner == pool PDA` (src line ~350). Added a
regression to lock this anti-theft invariant (it was previously untested):
subledger.rs::init_pool_rejects_a_vault_not_owned_by_the_pool.

### [BLOCKED] Distribution premature burn_unclaimed (DOS) — now pinned by a test
Probed: burn_unclaimed is permissionless. If it could run during the claim window
it would destroy claimants' COIN before they claim (DOS/LOF on every unclaimed
recipient). Already blocked: burn checks `clock.slot < window_end -> reject`
(src). The timing was only tested on the after-window side; added a regression for
the rejection side: distribution.rs::burn_unclaimed_is_rejected_during_the_claim_window
(burn at mid-window and at window_end-1 both rejected; recipients still claim in
full; burn permitted at/after window_end).

### [BLOCKED] TWAP on-chain attack surface — none yet
twap/ is a pure instruction-builder + math library (no entrypoint, no
process_instruction, no invoke). The buy/burn on-chain program (task #2) is not
built, so there is no live attack surface there yet. Re-examine once it lands.

### [FIXED] I. twap-program init didn't bind the controller multisig to the DAO
twap-program `init_config` validated the controller was OWNED by the Squads program
but never read its `config_authority`. So a TWAP could be wired to a real Squads
multisig whose config_authority is an attacker (not the DAO) — the config would
"look" DAO-governed while the DAO had no control (broken DAO->Squads link). FIX:
init now checks the controller is a Squads `Multisig` (disc) whose config_authority
(bytes [40..72]) == the named metadao_futarchy (DAO). Regression:
twap-program/tests/chain.rs (a multisig controlled by DAO but a different
metadao_futarchy passed -> rejected).

### [OPEN/NOTED] twap-program init permissionless + not genesis-bound
init_config is permissionless and per (market). It enforces controller-owned-by-
Squads + config_authority==DAO (finding I) but does NOT bind to the canonical
genesis: an attacker could still front-run with a self-consistent (their multisig,
their "DAO") config for the genesis market, bricking the legit TWAP setup or
controlling the future reconfigure path (same class as findings G/H). Proper fix
(follow-up): require squads_multisig == the deterministic genesis squads multisig
for coin_mint (derived via the rewards program's [b"genesis_squads", coin_mint]
create_key) and bind coin_mint/market to the genesis. Deferred until the
reconfigure/rotate instructions (which the controller actually gates) are built.

### [OPEN/NOTED] twap-program pull_surplus is permissionless + bps not enforced
IX_PULL_SURPLUS pulls a caller-specified `amount` bounded only by percolator's
WithdrawInsuranceLimited cap, NOT by the configured surplus_buy_burn_bps surplus
share. Pulled funds land in a twap_authority-owned holding (program-controlled, not
stealable), so no direct theft, but an over-crank could drain insurance beyond the
intended surplus share — relies on the post-mint insurance-policy rotation to
surplus-only mode. Bound the pull to the computed surplus share when the buy/burn
settlement slice lands (it needs to read market insurance-vs-backing state).

### [FIXED] J. Genesis Squads timelock was 48h, not the documented 1-week
The README documents a 1-week Squads timelock as THE user-exit backstop (Safety §3):
every genesis authority rotation runs DAO→Squads(1-week timelock)→..., leaving the
old authority in place for a week so users can exit before any rotation takes
effect. But `process_init_genesis_squads` created the multisig with
`TIMELOCK_48H_SECS` (48h), so the real exit window was ~2 days — a 3.5x shorter
safety window than promised, weakening the core backstop of the whole authority
chain. FIX: changed the on-chain genesis-squads timelock to one week
(TIMELOCK_1_WEEK_SECS = 7*24*60*60). squads_handover.rs updated so the enforcement
tests (M4/M5) prove execution is blocked until a FULL WEEK elapses (they advance the
clock by the renamed const). 4 squads tests green against real Squads v4.
NOTE: pinning init_genesis_squads's value end-to-end needs the drift-broken
integration harness; the timelock MECHANISM at 1-week is covered by M4/M5.

### [FIXED] K. Distribution didn't enforce the fixed-supply COIN (README §4)
README Safety §4 promises "the COIN mint has no mint authority ... no program can
mint COIN" — no inflation/dilution and no "mint to drain". But distribution
`init_config` validated the vault (mint/owner/funding, finding E) and never checked
the COIN mint's authority. So a distribution could be created against a
still-mintable COIN, and the mint-authority holder could mint unlimited COIN outside
the fixed pool, diluting every recipient's governance/value. FIX: init now unpacks
coin_mint and requires `mint_authority.is_none()` AND `freeze_authority.is_none()`
(a freeze authority could freeze the vault -> DOS all claims). The fixed pool is now
provably the entire COIN supply. Regression:
distribution.rs::init_config_rejects_a_mintable_coin (mintable -> rejected; after
revoking -> accepted). Cross-program tests updated to revoke the COIN authority
before dist init (matching the genesis-setup flow).

### [FIXED] H-overconstraint. genesis-vote init wrongly required subledger.mint == coin_mint
Finding H bound the subledger pool's mint to coin_mint. But the subledger holds the
at-risk COLLATERAL, a DIFFERENT mint from the distributed COIN (README money map);
finding K (fixed-supply COIN) made the conflict explicit — the COIN can't be the
mintable collateral. So H's mint check would REJECT a correct collateral != COIN
deployment (a self-inflicted DOS). FIX: dropped the `subledger_pool.mint ==
coin_mint` check from gv init_config; kept the security-critical binding
(vote_authority == this config PDA, findings G/H) which is what actually prevents a
poisoned/foreign pool. Cross-program tests refactored to use a separate fixed-supply
COIN (gv/distribution) vs the mintable collateral (subledger), as in the real design.

### [OPEN/DESIGN] L. Insurance exit is first-come under impairment, not pro-rata
Q: does the subledger correctly handle venue haircuts + surplus?
- SURPLUS: YES. percolator caps each WithdrawInsuranceLimited to
  `insurance*max_bps/1e4` then (deposits_only=1) `min(deposit_remaining)`, i.e. the
  deposited PRINCIPAL. Market profit/surplus is never withdrawable via the subledger
  exit. Correct ("never touch market profits", README §2).
- HAIRCUT: NO (first-come, not pro-rata). The cap tracks the LIVE insurance, and the
  withdraw also requires `amount <= vault` (percolator v16 ~line 8542/8555). The
  subledger requests the full `amount` (capped only to position.principal) and
  computes NO health-ratio haircut. So under an impairment (venue loss dropping
  insurance/vault below total deposited principal) the exit is FIRST-COME: an early
  depositor withdraws full principal and drains the impaired pool; a later one is
  stranded. This contradicts the documented "pro-rata under market loss / finalized
  withdraw haircuts by health ratio". Demonstrated against real percolator:
  insurance_percolator.rs::impaired_insurance_exit_is_first_come_not_pro_rata
  (alice exits whole + drains, bob stranded).
  NOT fixed — design decision needed: (a) accept first-come during the voting window
  with pro-rata only at a separate finalize path, or (b) make the subledger compute
  the haircut. (b) is non-trivial: it needs the LIVE asset-0 insurance figure
  (percolator's internal counter, not the vault token balance, which also holds
  backing), so the subledger would have to read the slab insurance accounting or
  percolator would expose a pro-rata withdraw. Flagged to the user.

### [FIXED] M. Proposal bait-and-switch (change distribution after voters back it)
distribution `append_entries` is allowed until seal, and genesis-vote
`register_proposal` did not freeze/snapshot the proposal. So a creator could register
a fair PARTIAL proposal, let voters back the gv_proposal, then append self-allocations
into the unallocated supply and trigger — sealing a distribution voters never
approved. Since the COIN IS the MetaDAO, that is a path to governance capture (bounded
only by the §3 timelock). FIX: register_proposal now snapshots the proposal's
(entry_count, total_amount) into the ProposalVote (and requires entry_count > 0 — only
a built proposal can be registered), and `trigger` re-reads the live proposal and
refuses to seal unless both match. So the sealed distribution is exactly the one
voters backed; any post-registration append breaks the snapshot and the seal is
refused. Regression (real cross-program e2e):
insurance_percolator.rs::proposal_changed_after_registration_cannot_be_sealed
(register partial -> vote -> creator appends -> trigger rejected).

### [BLOCKED] Percolator admin proxy (program/ IX_PERCOLATOR_ADMIN) — analyzed
Forwards percolator admin CPIs signed by the market_admin PDA. Well-guarded:
(a) tag ALLOWLIST (percolator_admin_tag_allowed) — only lifecycle-scoped tags;
(b) governance-authority gate (authority == coin_cfg.authority, signer);
(c) LOCKED ENTIRELY until genesis is finalized (the #16/#19 fix) — no pre-finalize
admin ops while depositor capital is at risk; kickstart/recovery use direct CPIs,
not this proxy. Post-finalize the allowlist is the MetaDAO's intended lifecycle
controls. Even the allowed UPDATE_INSURANCE_POLICY can't drain principal: the
subledger operator caps every withdraw to position.principal regardless of the
market policy. No gap. (Hard to e2e — needs the drift-broken genesis lifecycle.)

### [BLOCKED] Distribution seal authority — gated + already tested
seal_winner requires `authority.is_signer && authority.key == config.authority`
(the genesis-vote config PDA). A non-authority sealing would bypass the vote
entirely (governance capture) — already pinned:
distribution.rs::seal_then_recipients_claim_their_entries asserts an imposter
cannot seal and the real authority can.

### [BLOCKED] governance/ adapter forwarding (init/handover squads, percolator admin)
The adapter CPIs the rewards program signed by a PDA derived FROM the passed
rewards_program ([b"rewards_authority", rewards_program, coin_mint]). A malicious
rewards_program only ever yields a PDA bound to itself (no impersonation of the
legit authority), and the real validation (governance authority, finalized lock)
lives in the rewards program the adapter forwards to. Trusted-forwarder pattern,
PDA-bound. No gap.

### [COVERAGE] Full genesis lifecycle e2e (all real programs) — pinned
No test ran the COMPLETE chain to a COIN claim; a broken link (e.g. the cross-program
seal producing a non-claimable distribution) would brick the genesis. Added a full
e2e with percolator + subledger + genesis-vote + distribution all loaded:
collateral deposit -> vote -> permissionless trigger seals the winning distribution
by CPI -> winning recipient CLAIMS the fixed-supply COIN -> double-claim refused.
insurance_percolator.rs::full_lifecycle_deposit_vote_seal_then_recipient_claims_coin.
This pins that the genesis actually yields a claimable distribution and exercises the
separate collateral-vs-COIN mints (finding K) through the whole flow.

### [BLOCKED] vote_weight arithmetic overflow (genesis-vote)
`vote_weight = floor(log2(age)) * principal` uses `saturating_mul` (no wrap/panic)
and accumulation uses `checked_add` (graceful error). Saturating to u64::MAX needs
~2^58 real deposited tokens — self-bounded by capital, not attacker-reachable. No
fix/test needed.

### [BLOCKED] Subledger insurance_deposit holding-account substitution
`process_insurance_deposit` does not validate the `holding` account, but the
TopUpInsurance CPI's internal SPL transfer is authorized by the pool PDA, so
percolator requires `holding` to be pool-PDA-owned; a hostile holding makes the CPI
revert (whole tx reverts, user's step-1 transfer with it). The user pre-funds
`holding` with exactly `amount` and is credited `amount` — no path credits more
than entered or touches another user. Well defended; no test added.

### [BLOCKED] Subledger pool/position substitution in genesis-vote `vote`
`vote` pins `sub_pool == config.subledger_pool`, derives the position PDA from
that pool + voter, re-checks the stored pool/owner, and requires subledger-program
ownership. A foreign high-principal position cannot be substituted. Well defended;
no test added (would only re-assert existing checks).

### [FIXED] C. Pool type-confusion: own-vault path accepted an insurance pool
The subledger serves both own-vault pools (tags 1/2, funds in a pool-PDA-owned
vault) and percolator-insurance pools (tags 4/5, funds in the percolator
insurance vault). The insurance handlers already gated on `!pool.is_insurance()`,
but the own-vault `deposit`/`withdraw` had NO matching guard. Attack/footgun:
call own-vault deposit (tag 1) on an insurance pool — the SPL transfer pushes the
user's funds straight into the percolator insurance vault with NO TopUpInsurance
CPI (percolator never counts them) and records an own-vault position; the own-vault
withdraw can never sign those funds back out (the pool PDA is not the insurance
vault's token authority) → principal STRANDED (user LOF). FIX: added the symmetric
`if pool.is_insurance() { return Err }` guard to own-vault deposit AND withdraw.
Test (KEPT — pins a real stranded-funds boundary against the real percolator
binary): subledger/tests/insurance_percolator.rs::
own_vault_deposit_is_rejected_on_an_insurance_pool.

### [FIXED] D. Active-path canonical-vault pin (GH issue #24 / PR #25)
GH issue #24 + PR #25 (@SrMessiSOL) report that percolator-meta accepts a
non-canonical Percolator vault. ADVERSARIAL REVIEW of PR #25: the diff derives
the canonical ATA `[vault_authority, spl_token::ID, mint]` under the REAL ATA
program id and equality-checks it — byte-for-byte identical to percolator's own
`canonical_vault_address` (F-VAULT-FRAG). It is ADDITIVE (keeps the existing
mint/owner check) and can never reject a vault percolator would accept (percolator
enforces the same pin), so NO DOS and NOT a backdoor. The PR is legitimate but
targets the DEPRECATED custodial program/, whose integration suite no longer
compiles on master OR the PR branch due to percolator interface drift
(WrapperConfigV16 authority fields renamed) — unrelated to the PR.
The same gap existed on the ACTIVE path: subledger init_insurance_pool checked
`owner == vault_authority` + mint but did NOT pin the canonical ATA address. Not
exploitable (a non-canonical pool is simply inert — every deposit/withdraw CPI
reverts with InvalidVaultAccount), but a fail-fast pin is correct. FIX: added
`canonical_vault_address` to subledger and pinned it in init_insurance_pool.
Test (KEPT): insurance_percolator.rs::init_insurance_pool_rejects_non_canonical_vault.

### [FIXED] N. Fixed-supply COIN: pre-mint dilution (finding K gap)
Finding K required mint_authority==None (no FUTURE minting) but not that the mint's
CURRENT supply equals the distributed pool. An attacker could pre-mint extra COIN to
themselves BEFORE revoking, then fund the vault with only total_supply — holding
undistributed COIN that dominates governance (the COIN IS the MetaDAO). FIX: init
also requires `mint.supply == total_supply`; with the vault-funding check (E) this
proves every COIN that exists is in the distribution vault. Regressions:
distribution.rs::init_config_rejects_a_mintable_coin (now also pre-mint extra ->
rejected) + init_config_accepts_a_fully_in_vault_fixed_supply_coin.

### [COVERAGE] Squads -> TWAP reconfigure: timelock-gated, proven e2e
The twap IX_RECONFIGURE is gated on the config's Squads multisig default vault PDA.
Adversarial review: the multisig is pinned at init (finding I: config_authority ==
DAO), the vault is derived from it, new_bps is bounded (0,10000], and a foreign
config only exposes its own multisig's vault — secure. Pinned both ways against the
REAL Squads v4 binary:
- negative (chain.rs): a random signer / the correct vault address WITHOUT a
  signature are both rejected.
- keystone (chain.rs::reconfigure_only_via_squads_vault_execute_after_timelock):
  the DAO proposes a vault-transaction that CPIs reconfigure; executing it BEFORE the
  1-week timelock is rejected (config unchanged); after warping past the timelock the
  execute succeeds and the buy/burn share changes. This proves the on-chain
  DAO -> Squads(1-week) -> TWAP control link end-to-end and that the timelock cannot
  be bypassed for a TWAP reconfigure.

### [COVERAGE] Handoff bridge (twap IX_ACCEPT_OPERATOR) + dual-sign insight
Security insight: percolator UpdateAssetAuthority (tag 65) requires BOTH the current
authority (asset_admin) AND the new authority to SIGN — a safety feature preventing
rotation to a non-consenting/dead key. So the handoff to twap_authority cannot be a
plain squads-execute of UpdateAssetAuthority; it needs a twap-program bridge that
co-signs as twap_authority. Built IX_ACCEPT_OPERATOR: gated on the config's Squads
vault (the asset_admin, reachable only via a timelock'd execute), it CPIs percolator
UpdateAssetAuthority(asset 0, INSURANCE_OPERATOR, new=twap_authority), co-signing as
twap_authority via invoke_signed. The squads-vault gating is pinned (chain.rs: a
non-vault signer cannot trigger the rotation; same proven gate as the reconfigure
keystone). NEXT SLICE: the positive real-percolator e2e — a market with asset_admin
= the squads vault, squads execute -> accept_operator -> operator rotates
subledger->twap, and the subledger can no longer withdraw.

### [COVERAGE/OPS] Percolator interface coupling — drift caught + handoff CPI verified
The genesis is tightly coupled to percolator's exact per-instruction account lists.
A percolator .so rebuild mid-session silently broke EVERY subledger insurance CPI
with NotEnoughAccountKeys (the deployed binary briefly wanted more accounts); a later
rebuild restored it. Operational risk: a percolator upgrade can break the insurance
CPIs in lockstep — the subledger/twap CPIs must be re-synced on any percolator
interface change. Now that percolator is back in sync, pinned the TWAP handoff
bridge's percolator CPI encoding against the REAL binary:
insurance_percolator.rs::percolator_update_asset_authority_operator_encoding_is_accepted
(UpdateAssetAuthority tag 65, asset 0, kind INSURANCE_OPERATOR=2, [current(signer),
new(signer), market(w)] accepted). This both verifies accept_operator's CPI and acts
as an early-warning canary for future percolator drift on that instruction.

### [BLOCKED] Insurance-policy change is marketauth-gated (no attacker drain-policy)
The handoff rotates the insurance policy (principal-only -> surplus-only) via
percolator UpdateInsurancePolicy (tag 33), gated on the GLOBAL marketauth
(handle_update_insurance_policy: expect_live_authority(cfg.marketauth, admin)). The
risk: if anyone could change the policy, they could set deposits_only=0,
max_bps=10000 and enable withdrawing ALL insurance principal (drain). Pinned against
the real binary (insurance_percolator.rs::percolator_update_insurance_policy_is_marketauth_gated):
the marketauth can set the policy (encoding accepted) and a NON-marketauth is
rejected. In the handoff the marketauth is the squads vault, so policy changes are
1-week-timelock-gated. Also a drift canary for the policy-rotation encoding.

### [COVERAGE] Slice 3 handoff e2e — DAO->Squads(1wk)->TWAP->percolator, four real binaries
The keystone §3 test, proving the dangerous operation (rotating percolator's asset-0
insurance operator away from the constrained subledger) is timelock-gated end to end.
twap-program/tests/chain.rs::handoff_rotates_operator_to_twap_only_after_timelock:
market-0 with marketauth = the squads vault; DAO proposes a vault-transaction that
CPIs twap IX_ACCEPT_OPERATOR (which CPIs percolator UpdateAssetAuthority, co-signing
as twap_authority); executing BEFORE the 1-week timelock is rejected; after warping
past it the nested squads->twap->percolator CPI succeeds and the operator rotates to
twap_authority. All four real binaries (squads v4 + percolator + twap + the chain).
This is the §3 user-exit backstop in action: any authority rotation is delayed a full
week, in the clear, with the old constrained authority live the whole time.

### [COVERAGE] Slice 3 policy-rotation handoff e2e (timelock-gated, four binaries)
The handoff's other half: rotating the insurance policy (principal-only ->
surplus-only) is timelock-gated end-to-end. A bad policy (deposits_only=0,
max_bps=10000) could enable draining principal, so it must run through the timelock.
twap-program/tests/chain.rs::handoff_rotates_insurance_policy_only_after_timelock:
squads-execute -> percolator UpdateInsurancePolicy with the squads vault as the
marketauth; blocked before the 1-week timelock, succeeds after. Both handoff
operations (operator rotation + policy rotation) are now proven §3-timelock-gated
through the full DAO->Squads->percolator path with the real binaries.

### [OPEN/DEPENDENCY] O. Handoff before surplus-floor -> twap can pull PRINCIPAL (LOF)
After the operator handoff (twap IX_ACCEPT_OPERATOR) the twap_authority is the
percolator asset-0 insurance operator, and pull_surplus is PERMISSIONLESS. pull_surplus
pulls a caller-specified `amount` bounded ONLY by percolator's WithdrawInsuranceLimited
policy, with NO surplus-floor (reserved_principal + retained_surplus_floor, README §5).
Under the genesis principal-only policy (deposits_only=1) percolator caps to DEPOSITED
PRINCIPAL — so if the operator is handed to the twap while any depositor principal
remains, anyone could crank pull_surplus and pull PRINCIPAL (not just surplus) into the
twap holding -> LOF for non-exited depositors. §3 mitigates (depositors exit during the
1-week window before the rotation executes), but a non-exiter is exposed.
Real fix (buy/burn slice, slice 4): pull_surplus must enforce a floor — pull at most
(live asset-0 insurance - reserved principal). That needs the live insurance figure
from the slab (NOT the vault token balance, which also holds backing) and the reserved
principal (the subledger pool outstanding). Until then: do NOT perform the handoff, and
the handoff proposal should atomically set the policy to surplus-mode. Documented with
loud SAFETY comments in twap-program pull_surplus + accept_operator. NOT a live vuln
(nothing deployed; handoff naturally follows the buy/burn build) but a sequencing LOF
risk recorded so it isn't missed.

### [O-update] Surplus-floor implementation blocker (slice 4)
Attempted finding O's fix: have pull_surplus compute surplus = insurance - reserved
and pull at most that. percolator exposes the clean accessor
`percolator_prog::state::read_market(slab) -> (WrapperConfigV16, MarketGroupV16)`
with `wrapper.insurance_withdraw_deposit_remaining` (reserved principal) and
`group.header.insurance` (live insurance). BUT adding percolator-prog as a twap-program
LIB dep FAILS the SBF build — it pulls in `wincode-derive`, whose manifest doesn't
parse on the BPF toolchain (`failed to parse manifest ... wincode-derive-0.4.6`). So
the clean in-program read is blocked. Options for slice 4:
(a) percolator exposes a lightweight, wincode-free accessor for (insurance,
    deposit_remaining) the twap can link; or
(b) the twap reads raw slab offsets for those two u128s — but the slab layout drifts
    (proven this session), so this needs a pinned percolator + a layout canary.
Until resolved, finding O stands: do NOT perform the handoff, and the loud SAFETY
comments in pull_surplus/accept_operator remain. Reverted the read_market attempt;
twap-program stays green.

### [STATE] Audit coverage summary (live code exhaustively swept)
78 tests green across all crates. Findings A-O recorded. The live/built on-chain code
(subledger, genesis-vote, distribution, twap-program) has been swept across every
guidance category and found hardened:
- account substitution / PDA-seed confusion: pool/position/ballot/proposal/config all
  PDA-pinned + disc-checked; no cross-substitution (A,F,H, vote/trigger pins).
- missing signer/owner checks: owner-bound exits, set_vote_lock owner-signer (G),
  seal authority-gated, reconfigure/accept_operator squads-vault-gated.
- account reinit: every init checks lamports/data_len==0; no account is ever closed,
  so no close-then-reinit.
- arithmetic over/underflow: checked_add/sub on all tallies/balances; vote_weight
  saturating; u128 for quorum/majority.
- type confusion: every account carries + checks an 8-byte discriminator.
- unchecked CPI: token_program pinned to spl_token::ID; percolator CPIs go to the
  config-pinned program; squads vault gating; the percolator-side authority is
  enforced by percolator (adversarially pinned: operator + policy rotations).
- rent/closing, sysvar spoofing: no account closing; Clock via syscall (not passed).
- bait-and-switch, vote-outlives-capital, hostile-authority freeze, fixed-supply,
  drain-policy, minority-capture: all fixed (A,B,B2,G,K,M,N) + real-binary regressions.
Drift canaries (insurance_percolator.rs) pin every percolator CPI encoding the code
uses (TopUp/Withdraw/UpdateAssetAuthority/UpdateInsurancePolicy) against the live .so.
OPEN, build-gated: finding O (surplus floor, needs a wincode-free percolator accessor
for slice 4) and the twap genesis-binding front-run (needs slice 2's squads-creation
port). No live vuln remains in built code.

### [O-update 2] Surplus-floor: confirmed needs a percolator-side accessor (precise ask)
Investigated the raw-offset alternative for finding O's floor. Result: NOT cleanly
doable on-chain.
- `insurance_withdraw_deposit_remaining` lives in WrapperConfigV16 (#[repr(C)] +
  bytemuck::Pod) at slab offset HEADER_LEN(16) + offset_of!(...): computable + stable.
- `insurance` lives in MarketGroupV16, which is #[cfg(not(target_os="solana"))] with
  Vec fields — i.e. the HOST deserialization, not the on-chain layout. The slab stores
  the group zero-copy with the Vecs serialized inline, so `insurance`'s slab offset is
  NOT computable from the struct (this is exactly why read_market/wincode exists), and
  read_market can't link into a BPF program (wincode-derive manifest fails on SBF).
PRECISE ASK for the percolator side to unblock slice 4's floor:
  add a target_os="solana"-compatible, wincode-free getter, e.g.
    pub fn read_asset0_insurance_and_reserved(slab: &[u8]) -> Option<(u128, u128)>
  returning (market_insurance_remaining(asset 0), insurance_withdraw_deposit_remaining)
  by reading the fixed scalars directly from the slab bytes. Then twap pull_surplus
  enforces amount <= insurance - reserved. Until then finding O stands (handoff must
  not run; SAFETY comments in place).

### [FIXED] M2. Front-run griefing DOS on register_proposal (genesis-vote)
Finding M takes a snapshot of (entry_count, total_amount) at REGISTRATION time and
`trigger` rejects the proposal forever if the live distribution proposal no longer
matches that snapshot. register_proposal was permissionless (only payer.is_signer),
so an attacker could register a creator's PARTIALLY-built distribution proposal,
freezing a stale snapshot; the creator's very next `append` would then make the live
proposal mismatch the sealed snapshot, and `trigger` would reject it permanently — a
front-run that bricks a legitimate proposal (it can never be sealed -> finalize DOS),
costing the attacker only the gv_proposal rent.
Fix (genesis-vote/src/lib.rs register_proposal, in the snapshot block after the
config-binding check): bind registration to the distribution proposal's CREATOR
(header [48..80]) — `if creator != *payer.key { return Err(IllegalOwner) }`. The
creator registers only once they have finished building, so no third party can freeze
a premature snapshot. Regression: subledger/tests/insurance_percolator.rs
`only_the_proposal_creator_can_register_it` (a non-creator signer is rejected; the
creator succeeds) against the real binaries. Green.

### [BLOCKED] Distribution claim/seal/burn — account-substitution swept, no new vector
Adversarially swept the distribution claim path (the COIN payout surface) for LOF/DOS
this tick. Confirmed hardened; no test added (would be tautological):
- pull-model impersonation: claim pins `entry.pubkey == recipient.key` (only the named
  recipient is paid) AND `recipient.is_signer`. Covered by
  `seal_then_recipients_claim_their_entries` ("cannot claim bob's entry").
- double-claim: the entry amount is zeroed after the transfer; re-claim hits
  `amount == 0`. Covered (same test).
- cross-config substitution (the two non-obvious paths, now reasoned explicitly):
  (a) feeding a FOREIGN sealed proposal into the real config is blocked by
      `config.sealed_proposal == *proposal_account.key` (the config names exactly one
      payable proposal); (b) pairing the REAL vault with an attacker-sealed proposal is
      blocked because `vault.key == config.vault` and the sealed_proposal both derive
      from the SAME config account — there is no way to mix a real vault with a foreign
      proposal. The vault-authority PDA is seeded by `config.coin_mint`, so each config
      can only sign for its own vault.
- proposal griefing: create_proposal binds `header.creator`, append_entries requires
  `header.creator == creator.key` (third parties cannot append to / brick a creator's
  proposal — distribution-side analogue of finding M2, already enforced here).
- window race: claim requires window OPEN, burn requires window CLOSED (no overlap);
  burn-during-window DOS pinned by `burn_unclaimed_is_rejected_during_the_claim_window`.
- supply/strand: init pins vault.amount >= total_supply, seal pins total_amount <=
  total_supply, so claims can never strand a late recipient. Covered by
  `init_config_rejects_an_underfunded_vault` / `append_cannot_exceed_total_supply`.
- OOB: capacity <= MAX_ENTRIES (10k), index < entry_count <= capacity, so
  entry_offset stays in-bounds; size math can't overflow.
Distribution suite green (lib 4 + integration 7).

### [COVERAGE] Winner-take-all is irreversible across COMPETING proposals
Swept the genesis-vote vote/trigger tally surface this tick (checked_add/sub on every
tally, ballot PDA-pinned per (config,voter), sub_position canonical-PDA + owner pinned,
quorum read LIVE from the pool). All hardened. Found one DISTINCT boundary with no
direct coverage and pinned it: the existing re-trigger test only blocks the SAME
proposal (via pv.executed). Two COMPETING proposals share ONE distribution config, and
post-execution VOTE_RETRACT is allowed (so voters can leave an executed A and shift
weight onto B) — so the genesis-vote layer alone does not guarantee a single winner.
The true winner-take-all gate is the distribution `seal_winner` is_sealed() check:
B's trigger passes every gv check, sets pv_B.executed, then the seal CPI fails because
the config is already sealed, reverting B's trigger whole. New test
(genesis-vote/tests/seal.rs `a_second_proposal_cannot_reseal_after_a_winner_is_sealed`):
A seals; B is then made to look winning at the gv layer; trigger(B) is rejected and the
sealed winner stays A. KEPT — pins the cross-proposal irreversibility (not the same as
the same-proposal re-trigger block). genesis-vote suite green (lib 3 + seal 4).
Also noted (not a live risk): total_cast_weight is u64 and each voter adds <= 63*principal;
the sum could in principle overflow u64 only at absurd aggregate principal (> ~2.9e17
base units genuinely at risk), where checked_add fails the marginal vote (no corruption,
just a failed late vote). Not worth a saturating change; recorded for completeness.

### [BLOCKED] Subledger fund-movement (deposit/withdraw, both pools) — swept, no new vector
Swept the actual principal-movement surface this tick for LOF/dilution/rounding theft.
Confirmed sound; no test added (would be marginal — the boundaries are already pinned):
- NO phantom-principal dilution: own-vault deposit (subledger.rs path) does a real
  owner-signed SPL transfer of EXACTLY `amount` owner_ata -> vault BEFORE
  `outstanding_principal += amount`; if the transfer fails the tx reverts. So recorded
  principal is always backed by funds in the vault — an attacker cannot inflate
  outstanding without funding it (which would otherwise shrink honest depositors'
  pro_rata = balance*principal/outstanding on impairment). Insurance deposit likewise
  moves funds via TopUpInsurance before bumping outstanding.
- Rounding favors the POOL, never the attacker: mul_div_floor floors every pro_rata
  payout, so a withdrawer always gets <= fair share; dust strands in the vault (not
  stealable — only the pool PDA can move vault funds). No repeated-deposit rounding
  drain.
- No impaired-pool first-mover advantage (own-vault): proven order-independent — each
  exit removes balance and outstanding proportionally, leaving the remaining ratio
  unchanged. Pinned by `impaired_pool_is_pro_rata_and_order_independent` (alice-first,
  asserts bob gets the SAME 50% haircut, not worse). Donating to the vault only RAISES
  others' payout; an attacker cannot remove from the vault. No sandwich.
- Haircut/surplus coverage is complete: own-vault fair pro-rata + surplus
  (`with_surplus_policy_returns_yield_pro_rata`); insurance first-come
  (`impaired_insurance_exit_is_first_come_not_pro_rata`, finding L, documented);
  payout() pure-fn unit tests cover healthy/impaired/with-surplus/guards both ways.
- Owner/PDA pins: every deposit/withdraw re-derives the pool PDA + bump, pins
  vault==pool.vault, position==canonical PDA, owner-signer + position.owner==owner.
  Covered by `principal_only_owner_exit_returns_funds_and_guards` and
  `init_pool_rejects_a_vault_not_owned_by_the_pool`.
Subledger suites green (own-vault subledger.rs 5 + insurance_percolator 15).

### [FIXED] P. init_config front-run squat -> permanent deployment DOS (twap-program)
twap-program init_config is PERMISSIONLESS and its bindings (squads_multisig,
metadao_futarchy, coin_mint, percolator_program) were caller-supplied with only an
INTERNAL consistency check (squads_multisig owned by Squads + its config_authority ==
metadao_futarchy). The config PDA was keyed on `market_slab` ALONE. So an attacker could
stand up a throwaway Squads multisig (config_authority = an attacker key they also name
as metadao_futarchy — cheap, one multisig_create_v2), pass the consistency check, and
init the per-market config FIRST with their own bindings. The squatted config is inert
(accept_operator/reconfigure are gated on the squatted multisig's vault, which is NOT
the market's asset_admin, so it can never rotate the real operator) — but the per-market
config PDA is now TAKEN and cannot be re-initialized (AccountAlreadyInitialized), so the
real DAO's buy/burn deployment for that market is permanently bricked. A post-genesis,
no-fund-loss, permanent griefing DOS; the market_slab is public so the front-run is easy.
Fix: fold the caller-set bindings into the config PDA seed — now
[CONFIG_SEED, market_slab, squads_multisig, coin_mint, percolator_program]. The legit
config PDA = f(market, real_ms, real_coin, real_perc); to land an account THERE an
attacker must pass exactly those, which forces the real metadao_futarchy (via the
config_authority check) and yields the CORRECT config (no harm). Any attacker variation
lands at a different PDA the real deployment ignores. No percolator slab read needed (so
NOT blocked on finding O's accessor), no owner check (keeps fake-market unit tests valid).
reconfigure/accept_operator/pull_surplus never re-derive the config PDA (they trust
owner==program_id + bindings), so the seed change is safe for them. Regression:
twap-program/tests/chain.rs `init_config_front_run_with_attacker_multisig_cannot_block_the_real_deployment`
(attacker front-runs with their own multisig; lands at a different PDA; the real DAO's
init still succeeds and the live config is bound to the real multisig/DAO). Against the
real Squads v4 binary. twap-program suite green (lib 2 + chain 5).

### [FIXED] Q. init_insurance_pool front-run squat -> genesis deposits routed to attacker market (LOF)
Same class as finding P, worse impact. subledger init_insurance_pool is PERMISSIONLESS
and its market binding (market_slab, percolator_program, vault, vote_authority) was
caller-supplied; the pool PDA was keyed on (mint, asset_id) ALONE. The genesis pool PDA
= f(COIN_mint, 0) and the gv config PDA = f(COIN_mint) are BOTH predictable. So an
attacker could:
 1. stand up their own percolator market MKT_a (they are its marketauth) with the
    subledger pool PDA pre-set as MKT_a's asset-0 insurance authority/operator;
 2. front-run init_insurance_pool: pool=f(COIN_mint,0), market_slab=MKT_a, vault=MKT_a's
    canonical insurance vault, vote_authority = the predictable real gv config PDA,
    which PASSES init (vault is MKT_a's canonical vault) and later PASSES the gv
    init_config binding check (pool.vote_authority == gv config PDA).
Genesis then wires to a pool that routes every depositor's TopUpInsurance into MKT_a.
The attacker (MKT_a's marketauth) can then strand or bleed that insurance (hostile
policy / engineered market loss to their own trading account): depositor LOF, not just a
setup DOS. The real orchestrator's pool init also fails (PDA taken) = DOS even without
the LOF escalation.
Fix: fold market_slab + percolator_program into the pool PDA seed — now
[b"subledger_pool", mint, asset_id, market_slab, percolator_program]. The genesis pool
address can only ever hold a pool bound to the real market; an attacker's pool (any other
market) lands at a different PDA the genesis ignores. Own-vault pools pass Pubkey::default()
for both seed components (matching what they store), so the own-vault path is unchanged.
Threaded through every pool derive + invoke_signed seed set (init_pool, withdraw,
init_insurance_pool, insurance_deposit, insurance_withdraw). genesis-vote is unaffected
(it never derives the pool PDA; it trusts the stored config.subledger_pool key). No
percolator slab read needed (NOT blocked on finding O). Regression:
subledger/tests/insurance_percolator.rs `init_insurance_pool_cannot_be_squatted_to_misdirect_the_genesis_pool`
(attacker inits a pool bound to their own market against the real percolator binary; it
lands at a different PDA; the genesis pool still inits and binds the REAL market). On the
old seed the attacker pool PDA would equal the genesis pool PDA — the assert_ne + the
genesis re-init would both fail, so the test genuinely catches the regression. All suites
green: subledger lib 6 + insurance 16 + own-vault 5; genesis-vote 3+4; distribution 4+7.
NOTE: this is the same permissionless-init + caller-bindings + too-few-seeds pattern as
finding P (twap). Pattern to watch in any future init: bind the PDA to ALL trust-relevant
caller-supplied accounts, or land squats at harmless distinct addresses.

### [FIXED] R. gv init_config front-run squat -> genesis bound to attacker pool (LOF/DOS)
Third instance of the permissionless-init squat pattern (after P/Q). genesis-vote
init_config is permissionless and its config PDA was keyed on COIN_mint ALONE. It binds
a distribution_config and a subledger_pool, each required to point back at the (then
predictable) gv config PDA. The distribution_config is a UNIQUE PDA f(COIN) that can't be
forged (distribution init needs the funded fixed-supply COIN), so it can't be substituted
-- but the subledger_pool is NOT unique: an attacker could create their own valid pool
(vote_authority = the predictable gv PDA, bound to a market they control post-finding-Q)
and FRONT-RUN init_config to bind the genesis to THAT pool. Then every depositor's
principal routes into the attacker's pool/market (LOF), or the quorum is read from the
wrong pool (DOS), and the real orchestrator's init fails (PDA taken).
Fix: fold subledger_pool into the gv config PDA seed -> [b"gv_config", COIN_mint,
subledger_pool]. The legit gv address = f(COIN, real_pool); an attacker's pool yields a
different gv PDA the genesis ignores. And because the unique distribution_config's seal
authority is pinned to ONE gv PDA, post-fix only the pool that distribution commits to can
ever be bound -- a substituted pool makes `expected` mismatch the distribution authority
and is refused. distribution_config is NOT in the seed (it is already unique per COIN, so
unsubstitutable). Threaded through init derive + create_pda + the vote() and trigger()
gv-config signing seeds (config.subledger_pool). No percolator slab read (not blocked on
finding O). Tests: seal.rs Env derives gv config after the pool; new
`gv_config_cannot_be_bound_to_a_substituted_pool` (asserts the gv PDA now commits to the
pool -- fails pre-fix -- and that binding a substituted attacker pool is rejected by the
distribution-authority pin). All suites green: genesis-vote 3+5; subledger 6+16+5.
PATTERN now 3x (P twap, Q subledger, R gv): permissionless init + caller-supplied
bindings + PDA keyed on too few seeds. RULE: an init PDA must commit to every
trust-relevant caller-supplied account that is not already unique/unforgeable, so squats
land at harmless distinct addresses. Remaining permissionless init reviewed: distribution
init_config keys on COIN_mint and binds the COIN mint + vault (validated funded/fixed
supply) -- the COIN mint is the identity, the vault/authority are caller-set but a squat
there only produces a config for that same COIN (no misroute), and a re-init is blocked;
acceptable. twap init_config = finding P (fixed). subledger init_insurance_pool = finding
Q (fixed).

### [COVERAGE] Grand-unified E2E: genesis -> handoff -> twap surplus pull (ALL six binaries)
Built the full end-to-end lifecycle in ONE litesvm instance against every real binary
(subledger, genesis-vote, distribution, percolator, Squads v4, twap-program):
twap-program/tests/chain.rs `e2e_full_genesis_to_twap_surplus_pull`.
Flow: market-0 with marketauth = the Squads vault (asset_admin) -> DAO/Squads (1-week
timelock) injects insurance SURPLUS via percolator TopUpInsurance -> DAO/Squads grants the
asset-0 insurance authority+operator to the subledger pool (subledger.accept_operator, the
pool only CONSENTS) -> a depositor tops up REAL percolator insurance through the subledger
-> fixed-supply COIN distribution set up + the genesis vote/trigger seals the winning
distribution by CPI -> the winner CLAIMS the COIN -> DAO/Squads rotates the insurance
policy to surplus-mode AND the operator to the twap (both timelock-gated) -> the twap, now
the asset-0 insurance operator, pull_surplus pulls exactly the surplus, leaving depositor
principal in insurance. Asserts: insurance = surplus+principal after deposit; winner gets
the full COIN; twap holding receives the surplus; principal remains.
Design honored: the subledger NEVER rotates keys. Squads (driven by the DAO) is the
asset_admin and the only key-rotator; subledger + twap are pure insurance fund-managers
that only consent (accept_operator) to receive the operator role. The added subledger
accept_operator (tag 7) is the mirror of the twap's, required solely because percolator's
asset-0 UpdateAssetAuthority has no consent-free grant path (the incoming key must co-sign).
Stage-A (`e2e_squads_grants_operator_to_subledger_then_real_deposit`) pins the grant+deposit
half on its own. All suites green: twap lib 2 + chain 7; subledger 6+16+5; gv 3+5; dist 4+7.

### [BLOCKED] E2E probe: operator grant cannot bypass the Squads timelock
ATTACK: an attacker calls subledger.accept_operator DIRECTLY (not through a Squads
execute), signing as a forged asset_admin, to grant the asset-0 insurance operator to the
pool outside the 1-week timelock. The pool consents (its PDA is hardcoded in
accept_operator), but the inner percolator UpdateAssetAuthority requires the signer to be
the asset-0 asset_admin (the Squads vault) — an attacker key (and the plain payer) is not,
so percolator rejects. Confirms the grant/rotation is reachable ONLY through the real
asset_admin, i.e. a timelock'd Squads execute; calling the subledger straight cannot
sidestep it. accept_operator is powerless on its own — it only co-signs; percolator is the
gate. Test: twap-program/tests/chain.rs
`e2e_attacker_cannot_grant_operator_bypassing_squads` (real Squads v4 + percolator + the
deployed subledger). KEPT — pins the core authority boundary of the whole handoff.

### [OPEN/DEMONSTRATED] O-update 3. finding O proven end-to-end against real binaries (LOF)
ATTACK (finding O, now demonstrated): after the operator handoff to the twap with the
surplus-mode policy (deposits_only=0), a PERMISSIONLESS cranker calls pull_surplus and
drains DEPOSITOR PRINCIPAL even when there is ZERO surplus. Built end-to-end on the real
binaries: Squads funds insurance with 1,000,000 of PURE principal (no surplus), hands the
operator to the twap, then a cranker pulls 800,000 (80% = the surplus-mode max_bps cap) —
and it SUCCEEDS, moving principal into the twap holding. percolator surplus-mode caps to
max_bps of insurance and reserves nothing (it even zeroes insurance_withdraw_deposit_remaining),
so the floor MUST be enforced twap-side, which is not yet built. Test:
twap-program/tests/chain.rs `e2e_finding_o_cranker_drains_principal_no_floor` — it asserts
the drain currently SUCCEEDS (a KNOWN-OPEN marker); flip it to assert rejection once
pull_surplus enforces `amount <= insurance - reserved`.
Confirmed the fix is still blocked from this repo: the surplus floor needs the slab's
asset-0 `insurance` figure. The canonical insurance vault token balance is NOT a usable
proxy (it is the market's shared collateral vault, not insurance-only), so the reserved
amount cannot be derived from (vault_balance - subledger_outstanding) on-chain. Needs the
percolator-side wincode-free accessor read_asset0_insurance_and_reserved (findings O,
O-update 2). Until then: DO NOT perform the handoff in production with depositor principal
still present.

### [FIXED] O. Surplus floor — twap reads the slab insurance directly (no accessor needed)
Finding O is fixed. The earlier "blocked on a percolator wincode-free accessor" framing was
wrong: Solana account data is globally readable, so the twap reads the asset-0 `insurance`
u128 STRAIGHT FROM THE MARKET SLAB bytes. The slab's zero-copy MarketGroupV16 header is a
repr(C) Pod of [u8;N] newtypes (align 1, no padding) at MARKET_GROUP_OFF =
HEADER_LEN(16)+WRAPPER_CONFIG_LEN(432)=448; `insurance` sits at +285 within it (after
market_group_id 32 + V16ConfigAccount 233 + asset_slot_capacity 4 + vault 16). So
INSURANCE_OFFSET = 733, pinned by the `insurance_offset_matches_real_percolator_slab` canary
(funds insurance with a unique value via a real Squads TopUp and asserts the slab bytes match;
fails loudly on layout drift).
Fix: pull_surplus now enforces `amount <= insurance - reserved_floor`. `reserved_floor` is a
new twap Config field (the reserved depositor principal), initialized to u128::MAX (so a
freshly-configured twap pulls NOTHING) and lowered only by the DAO via a new Squads-vault-gated,
timelock'd `set_reserved_floor` (tag 4). A permissionless crank can therefore never reach
principal, regardless of percolator's policy mode. The loud SAFETY/ORDERING-DEPENDENCY comments
in pull_surplus/accept_operator are updated to reflect the bound.
Regression: `e2e_finding_o_floor_blocks_principal_drain` (was the known-open demonstration that
the drain SUCCEEDS) now sets the floor = principal and asserts the cranker's principal pull is
REJECTED with no funds moved. `e2e_full_genesis_to_twap_surplus_pull` sets the floor = principal
and pulls exactly the surplus (insurance - floor), leaving principal intact. All real binaries.
twap suite green: lib 2 + chain 10.

### [BLOCKED] E2E probe: the surplus floor cannot be lowered outside the Squads timelock
ATTACK (integrity of the finding-O fix): the reserved_floor is the only barrier between a
permissionless pull_surplus and depositor principal, so re-enabling the drain just means
lowering it. An attacker calls twap.set_reserved_floor DIRECTLY to drop the floor to 0:
(1) signing with their own key passed as the squads vault — rejected (key !=
squads_default_vault(config.squads_multisig)); (2) passing the REAL squads vault but as a
non-signer — rejected (it must be a signer, and the vault PDA can only sign inside a Squads
execute). The floor stays at its u128::MAX default. So the floor is lowerable ONLY through a
timelock'd DAO/Squads execute, exactly like reconfigure/accept_operator. Test:
twap-program/tests/chain.rs `e2e_attacker_cannot_lower_surplus_floor_without_squads`
(real Squads v4 + percolator + twap). KEPT — pins the integrity of the finding-O floor.

### [COVERAGE/SEQUENCING] E2E probe: the operator handoff closes the subledger exit path
The operator handoff to the twap is a POINT OF NO RETURN for the subledger exit. The
subledger insurance_withdraw signs as the pool, which is the asset-0 insurance OPERATOR only
until the handoff; afterwards the operator is the twap, so percolator rejects a pool-signed
WithdrawInsuranceLimited. Proven end-to-end: alice deposits, withdraws fine BEFORE the
handoff, then her withdraw is REJECTED AFTER the operator rotates to the twap. Implication: a
depositor who has not exited before the (1-week-timelock'd) handoff can no longer withdraw via
the subledger. Their principal is NOT stealable — the finding-O floor stops the twap pulling
it — but it is LOCKED until the DAO rotates the operator back to the subledger. This is a
liveness/DOS consideration, not a theft vector: it enforces the design's "exit during the
timelock window" requirement (README §3) and means a malicious DAO can at worst lock (not
steal) a non-exiter's principal, and only after a 1-week public warning. Test:
twap-program/tests/chain.rs `e2e_subledger_exit_blocked_after_operator_handoff`. KEPT — pins
the handoff sequencing boundary against the real binaries.

### [BLOCKED] E2E probe: looping pull_surplus cannot cumulatively cross the floor
ATTACK (finding O fix, cumulative drain): a cranker loops pull_surplus to drain principal in
pieces rather than one over-pull. Because pull_surplus re-reads LIVE asset-0 insurance from
the slab on EVERY call and caps to `insurance - reserved_floor`, successive pulls converge to
the floor and never cross it — even across the percolator withdraw cooldown. Proven
end-to-end: insurance = principal(1,000,000) + surplus(500,000), floor = principal; the cranker
pulls 250k, warps past the cooldown, pulls another 250k (draining exactly the surplus), then a
third pull of even 1 unit is REJECTED, with the full principal intact in insurance and exactly
the surplus in the twap holding. Pins that the floor is stateless/live (no cached insurance, no
cumulative over-pull). Test: twap-program/tests/chain.rs `e2e_floor_holds_across_repeated_pulls`.
KEPT — distinct from the single-pull finding-O test (this covers the looping/cumulative case).

### [BLOCKED] E2E probe: a cranker cannot redirect surplus to its own holding
pull_surplus is PERMISSIONLESS (anyone may crank it), so the destination must be locked to
the twap_authority — otherwise a cranker could pull the surplus into their own wallet (free
money). pull_surplus requires `holding.owner == twap_authority` (the percolator
WithdrawInsuranceLimited dest-owned-by-operator rule, re-checked twap-side). Proven
end-to-end: an attacker cranks pull_surplus with a holding token account THEY own; it is
rejected, no surplus reaches the attacker, and the insurance vault is untouched. So a
permissionless crank can only ever move surplus into a twap_authority-owned account (from
which only the twap program, via the buy/burn slice, can act). Test:
twap-program/tests/chain.rs `e2e_cranker_cannot_redirect_surplus_to_own_holding`. KEPT —
pins the destination boundary of the permissionless pull.

### [FIXED] S. Post-handoff deposits were drainable as "surplus" (LOF)
Found via the e2e probe loop. The handoff rotated only the asset-0 insurance OPERATOR (kind
2) to the twap, leaving the subledger pool as the insurance AUTHORITY (kind 1). So subledger
insurance_deposit (TopUp, gated on kind 1) STILL WORKED after the handoff. Because the twap's
reserved_floor (finding O) is a STATIC snapshot taken at handoff, a deposit made afterwards
raised the live asset-0 insurance ABOVE the floor — turning that new principal into pullable
"surplus". Demonstrated end-to-end against the real binaries: post-handoff a depositor topped
up 500,000, then a permissionless cranker pulled exactly 500,000 (insurance - floor) into the
twap holding — the depositor's entire principal drained.
Fix: twap.accept_operator now ATOMICALLY rotates the insurance authority (kind 1) to the
Squads vault in the same instruction it accepts the operator (kind 2). Both `current` and
`new` are the Squads vault (the asset_admin, propagated from the timelock'd execute), so it
needs no extra consent. After the handoff NOBODY can TopUp market-0 insurance, so no new
(unprotected) principal can enter and the static floor is sound. The subledger is fully
disconnected post-handoff (neither kind 1 nor kind 2) — consistent with "genesis is over".
Regression: twap-program/tests/chain.rs `e2e_post_handoff_deposit_blocked_by_authority_revoke`
(post-handoff deposit is now REJECTED; insurance stays exactly the genesis principal, nothing
drained). e2e_full_genesis_to_twap_surplus_pull still green (the revoke happens after all
genesis deposits). twap suite green: lib 2 + chain 15.

### [BLOCKED] E2E probe: a freshly-deposited position has no vote weight (anti flash-vote)
Vote weight = floor(log2(age)) * principal, where age = now_slot - position.start_slot
(last-write-time). A position deposited in the CURRENT slot has age < 2 → weight 0, and the
gv `vote` instruction rejects a zero-weight vote ("position has no vote weight"). So a voter
cannot flash-deposit, vote with full principal weight, and immediately exit — governance
influence costs real time-at-risk, not just momentary capital. Proven end-to-end against the
real subledger + genesis-vote binaries: alice deposits and votes in the SAME slot (REJECTED),
then after holding a few slots her vote SUCCEEDS. Complements (does not duplicate) the gv lib
unit test of vote_weight() — this exercises the full path: real subledger position ->
weight computation -> vote-instruction rejection. Test: twap-program/tests/chain.rs
`e2e_fresh_position_has_no_vote_weight`. KEPT — pins the time-at-risk requirement of the vote.

### [BLOCKED] E2E probe: pull_surplus is locked to the config's market vault (no cross-market drain)
pull_surplus moves funds out of the market insurance vault, so its SOURCE must be locked to
the config's market — otherwise a cranker could point the WithdrawInsuranceLimited at a
DIFFERENT market's vault/authority and drain another market's insurance. The twap pins
vault_authority == perc_vault_authority(config.market_slab) (and market_slab/percolator_program
== config). Proven end-to-end: a cranker passing a foreign market's vault_authority (derived
for a different slab) is rejected, and the insurance vault is untouched. So a permissionless
crank can only ever withdraw from THIS market's canonical insurance vault. Test:
twap-program/tests/chain.rs `e2e_pull_surplus_rejects_foreign_vault_authority`. KEPT — pins
cross-market source integrity. (Also added the `setup_handoff` harness helper to keep future
twap-side probes focused on the attack.)

### [BLOCKED] E2E probe: one vote, one proposal — cannot back two without retracting
A voter's weight = floor(log2(age)) * principal is their CAPITAL's say; backing more than one
proposal at once would split or double-count that capital across proposals (vote-splitting /
double influence). The gv `vote` enforces one-vote-one-proposal: while a ballot is LIVE on
proposal A, backing a DIFFERENT proposal B is rejected ("retract your existing vote before
backing another proposal") — the voter must retract A first, which frees the weight. Proven
end-to-end against the real subledger + genesis-vote binaries: alice backs A, is REJECTED
backing B, retracts A, then successfully backs B. This invariant was previously untested
anywhere. Test: twap-program/tests/chain.rs `e2e_voter_cannot_back_two_proposals_without_retracting`.
KEPT — pins one-vote-one-proposal. (Also added setup_genesis + register_proposal harness
helpers to keep future genesis-side probes focused.)

### [BLOCKED] E2E probe: no surplus pull before the floor is configured (fail-safe default)
The handoff is several timelock'd Squads executes and the surplus floor is set in its own
step. In the window AFTER the operator rotates to the twap but BEFORE set_reserved_floor — or
if the DAO never sets a floor — reserved_floor is its init default u128::MAX, so pull_surplus
computes surplus = insurance - MAX = 0 and a permissionless cranker can pull NOTHING. Proven
end-to-end: insurance is funded with genuine surplus, the policy + operator are handed to the
twap, the floor is left unset (verified == u128::MAX), and a cranker's pull is rejected with
the insurance untouched. So a handed-off-but-unconfigured twap is safe by default; the
multi-step handoff exposes no funds at any intermediate point. Test:
twap-program/tests/chain.rs `e2e_no_surplus_pull_before_floor_is_configured`. KEPT — pins the
fail-safe default of the floor.

### [COVERAGE/LIVENESS] E2E probe: post-handoff exit lock is RECOVERABLE (never permanent loss)
Extends the handoff-sequencing probe with the recovery half. After the operator rotates to the
twap, a non-exiter's subledger withdraw is blocked (the pool is no longer the operator) — but
the lock is NOT permanent: the DAO, via a timelock'd Squads execute, rotates the insurance
operator+authority BACK to the subledger pool (subledger.accept_operator, pool consents), and
the depositor then exits their principal. Proven end-to-end across the real binaries: exit
works BEFORE the handoff, is REJECTED after, and SUCCEEDS again after the DAO re-grant (the
previously-locked principal is recovered). Confirms the "principal is never permanently lost"
guarantee: the worst case for a non-exiter is a DAO-recoverable lock, never theft or burn. Test:
twap-program/tests/chain.rs `e2e_subledger_exit_blocked_after_operator_handoff` (now the full
exit lifecycle: works -> blocked -> recovered).

### [BLOCKED] E2E probe: minority turnout cannot capture the distribution (quorum guards turnout)
A minority-capital voter tries to seal their proposal by being the ONLY one to vote: they then
hold 100% of CAST weight (so the weighted-majority check passes trivially), but quorum is
total_voted_principal*2 > LIVE pool outstanding — measured against ALL deposited principal,
including non-voters. So a minority of live capital can never reach quorum. Proven with REAL
multi-party deposits against the real subledger + genesis-vote binaries: alice (400k of 1M
outstanding) votes and triggers -> REJECTED (no quorum), the distribution stays unsealed; only
once bob (600k) also votes does the trigger seal the winner. Distinct from the seal.rs
injected-tally quorum test — this exercises the full real path (two deposits -> outstanding ->
vote -> quorum) plus the positive case. Closes the "low-turnout capture" governance attack.
Test: twap-program/tests/chain.rs `e2e_minority_turnout_cannot_reach_quorum`. KEPT.

### [BLOCKED] E2E probe: a voter cannot vote with another voter's position (no vote-power theft)
Voting power is the voter's OWN capital. The gv `vote` derives the subledger position PDA from
the SIGNER (sub_position_seeds(pool, voter)) and pins the passed account to it, so a voter
cannot substitute someone else's (larger) position to vote with their weight. Proven end-to-end
against the real subledger + genesis-vote binaries: alice (100k) signs and passes BOB's (900k)
position account — REJECTED (the PDA derived from alice mismatches the passed account); alice
voting with her OWN position works. Closes the position-substitution / vote-power-theft vector.
Test: twap-program/tests/chain.rs `e2e_voter_cannot_vote_with_another_voters_position`. KEPT.

### [BLOCKED] E2E probe: only the winning proposal's recipients can claim (winner-take-all at claim)
Winner-take-all extends to the distribution claim: once proposal A is sealed as the winner, a
LOSING proposal's recipient gets NOTHING. The claim pins config.sealed_proposal (only the
winner pays) AND entry.pubkey == signer (pull model). Proven end-to-end with TWO real proposals
against the real binaries: a voter backs A to quorum+majority, the trigger seals A; A's named
recipient claims the full COIN supply; B's recipient (the loser) cannot claim from their own
never-sealed proposal B (config.sealed_proposal == A != B) NOR from the winner A (their pubkey
is not an entry there), and ends with zero. Distinct from the single-proposal distribution.rs
claim tests. Closes the "losing-proposal recipient claims anyway" vector. Test:
twap-program/tests/chain.rs `e2e_only_the_winning_proposal_can_be_claimed`. KEPT.

### [BLOCKED] E2E probe: capital dominates hold time — no early-squatter governance capture
Vote weight = floor(log2(age)) * principal: log-time is a SOFT, sub-linear (capped ~63)
multiplier while capital is LINEAR. So an early SMALL depositor cannot sit accumulating
time-weight to out-vote a later LARGE depositor and capture the COIN distribution cheaply.
Proven end-to-end with two competing proposals and real deposits: an early voter (100k held
~1500 slots, floor(log2)=10 -> weight ~1,000,000) backs proposal EARLY; a later voter
(1,000,000 = 10x capital, held ~16 slots, floor(log2)=4 -> weight ~4,000,000) backs proposal
LATE. The early-squatter proposal LACKS a weighted majority (1M*2 <= 5M cast) and cannot seal;
the larger-capital proposal IS the majority and seals. Confirms the Sybil-resistance balance —
capital (the at-risk cost) decides, with hold-time only a tie-tilting bonus. (Stayed inside the
percolator oracle-staleness window so deposits remain Live.) Test: twap-program/tests/chain.rs
`e2e_capital_outweighs_hold_time_no_early_squatter_capture`. KEPT.

### [BLOCKED] E2E probe: retract/re-back cannot inflate vote weight (tally integrity)
A voter cycles back -> retract -> re-back on the same proposal, trying to make their
support_weight accumulate beyond their single capital contribution (a weight-inflation /
double-count attack on governance influence). The gv `vote` subtracts EXACTLY the stored ballot
weight on retract (checked_sub on both the proposal support and the global total_cast_weight)
and re-adds a single fresh contribution on back — it never accumulates. Proven end-to-end:
after the first back, support_weight == total_cast_weight == W (one contribution); across 5
back/retract cycles (age fixed so weight is constant) retract returns both to 0 and re-back
restores exactly W — never 2W. Closes the cycling-inflation vector. Test:
twap-program/tests/chain.rs `e2e_retract_reback_cannot_inflate_vote_weight`. KEPT.

### [STATE] E2E attack-probe coverage map (genesis -> handoff -> twap, all six real binaries)
The probe loop has systematically swept the end-to-end chain. Full repo green (~98 tests):
subledger 6+16+5, genesis-vote 3+5, distribution 4+7, twap lib 24, twap-program lib 2 + chain 24,
setup 1+1 — no regression from this session's reseeds (P/Q/R), accept_operator, or finding O/S.
REAL bugs found + fixed by the loop:
  - O: pull_surplus had no surplus floor (LOF). Fixed by reading asset-0 insurance straight from
    the slab (offset 733, canary-pinned) and capping to insurance - reserved_floor (DAO-set,
    timelock'd, default u128::MAX).
  - S: post-handoff deposits were drainable (the pool kept kind-1 authority). Fixed by
    accept_operator atomically rotating kind 1 to the Squads vault, disabling deposits at handoff.
Boundaries pinned end-to-end (twap-program/tests/chain.rs), all BLOCKED:
  AUTHORITY/HANDOFF: operator grant cannot bypass the Squads timelock; reserved_floor cannot be
    lowered outside the timelock; no surplus pull before a floor is configured (fail-safe MAX
    default); foreign vault_authority rejected (no cross-market drain); cranker cannot redirect
    surplus to its own holding; repeated pulls cannot cumulatively cross the floor; finding-S
    deposit revoke; finding-O floor blocks principal drain.
  LIVENESS: post-handoff subledger exit is blocked but DAO-recoverable (principal never
    permanently lost).
  GOVERNANCE/VOTE: fresh (age<2) position has no weight (anti flash-vote); one vote, one
    proposal (no vote-splitting); minority turnout cannot reach quorum (no low-turnout capture);
    a voter cannot vote with another's position (no vote-power theft); capital dominates soft
    log-time weight (no early-squatter capture); retract/re-back cannot inflate weight.
  CLAIM: only the winning proposal's recipients can claim (winner-take-all at claim).
The high-value external-attack surface on the BUILT code is exhaustively covered. Remaining open
items are design/operational, not code bugs: finding L (impairment first-come vs pro-rata,
awaiting a design decision) and the unbuilt COIN buy/burn settlement slice (future probe target
once built). Future ticks: re-verify on any code change; target new instructions when added.

### [BLOCKED] E2E probe: a completed Squads execute cannot be replayed
The DAO->Squads->percolator handoff is a sequence of timelock'd vault transactions. A replay
attack: after a vault transaction executes (e.g., the asset-0 operator rotation to the twap),
re-run the SAME transaction to re-trigger the timelock'd action without a fresh
proposal/approval/timelock. Squads marks the proposal Executed, so a second execute of the same
transaction is rejected. Proven end-to-end on a fully handed-off market: the operator-handoff
vault transaction (idx 3) is re-executed and REJECTED. Confirms our multi-step handoff is
one-shot per step — a completed timelock'd action can never be replayed. (Our steps are also
idempotent/self-limiting, but this pins the squads replay protection in the integration.) Test:
twap-program/tests/chain.rs `e2e_completed_squads_execute_cannot_be_replayed`. KEPT.

### [BLOCKED/RECOVERY] E2E probe: twap tracks the live floor across impairment + recovery
pull_surplus reads LIVE asset-0 insurance every call, so the floor is enforced dynamically in
BOTH directions. Pinned end-to-end: pull the full surplus down to the floor (insurance ==
reserved_floor == principal); at the floor further pulls are BLOCKED (surplus 0); the DAO then
refills insurance above the floor (a TopUp via the Squads vault, which holds kind-1 post-handoff
— or, equivalently, market profits refilling it); the twap RESUMES pulling exactly the recovered
surplus and STILL cannot cross the floor; it ends back at the floor with the principal fully
intact and total pulled == original surplus + recovered surplus. This is the "recovers after
h<1, then healthy again" behaviour — the floor never lets a permissionless cranker touch
principal whether insurance is dropping toward it or recovering above it, and the twap pre-empts
nothing. Test: twap-program/tests/chain.rs `e2e_twap_resumes_pulling_after_insurance_recovers`.
KEPT.

### [BLOCKED] E2E probe: cannot vote without a subledger position (no free governance power)
Governance power is capital-at-risk: a voter must have a real subledger insurance position to
vote. The fresh-position probe covers a deposited-but-age<2 position (weight 0); this covers the
extreme — an account that NEVER deposited has no position account, so the gv `vote` cannot
read/own-check it (sub_position.owner != config.subledger_program for an uninitialized account)
and rejects. Proven end-to-end against the real subledger + genesis-vote binaries: an attacker
who deposited nothing tries to vote -> rejected; their position PDA is empty. So you cannot buy
governance influence without putting capital at risk. Distinct from e2e_fresh_position_has_no_
vote_weight (account-doesn't-exist vs weight-0 paths). Test: twap-program/tests/chain.rs
`e2e_cannot_vote_without_a_position`. KEPT.

### [BLOCKED] E2E probe: Sybil-splitting capital gives no vote advantage (core resistance property)
The whole premise is "Sybil-resistant governance": influence must scale ONLY with capital
at risk, not with the number of identities. Vote weight = floor(log2(age)) * principal is LINEAR
in principal, so splitting capital across many positions yields the SAME total as one large
position. Proven end-to-end against the real binaries: an attacker splits 1,000,000 into 4
identities of 250,000, all deposited at the same slot and voting the same proposal at the same
age (16, log2=4); the proposal's total support_weight == 4,000,000 == exactly what a single
1,000,000 position would produce, and the quorum denominator (total_voted_principal) == 1,000,000
(summed, not multiplied). So Sybiling neither inflates weight nor quorum — capital is the only
lever. This is the foundational property the design rests on; it was previously unpinned. Test:
twap-program/tests/chain.rs `e2e_sybil_splitting_gives_no_vote_advantage`. KEPT.

### [BLOCKED] E2E probe: exactly half the capital does not meet quorum (strict-inequality edge)
Quorum is total_voted_principal*2 > outstanding — a STRICT inequality. So a voter (or set) holding
EXACTLY half the live capital cannot seal: a 50/50 situation needs strictly MORE than half to have
voted. If this were >= a tie could capture the distribution. Pinned end-to-end against the real
binaries: two equal 500,000 depositors (outstanding 1,000,000); with only one voting (500k*2 ==
1,000,000, not >) the trigger is REJECTED for lack of quorum, and only once the second also votes
(1,000,000*2 > 1,000,000) does it seal. Guards the strict > (vs >=) so a half-and-half split can
never seal. Distinct from the 40%-minority probe (this is the exact 50% edge). Test:
twap-program/tests/chain.rs `e2e_exactly_half_capital_does_not_meet_quorum`. KEPT.

### [BLOCKED] E2E probe: a 50/50 weight tie between proposals deadlocks (majority strict edge)
The winner needs support_weight*2 > total_cast_weight — a STRICT inequality. So two proposals
each holding EXACTLY half of the cast weight TIE: NEITHER can seal. If this were >= both could
seal at 50% (double-seal / ambiguous winner). The tie simply deadlocks until additional weight
breaks it — preserving a single, unambiguous winner-take-all. Pinned end-to-end against the real
binaries: two equal voters (500k each, same age) back competing proposals A and B; triggering A
is rejected AND triggering B is rejected (each has exactly half the cast weight). A third voter
(100k) backs A, tipping it over half; A now has a strict weighted majority and seals as the sole
winner. Complements the quorum strict-edge probe (this is the MAJORITY/cast-weight edge). Test:
twap-program/tests/chain.rs `e2e_tied_weight_between_proposals_deadlocks_until_broken`. KEPT.

### [BLOCKED/DESIGN] E2E probe: a non-voter's exit recomputes quorum (those who stay decide)
Quorum is total_voted_principal*2 > LIVE pool outstanding, so a passive holder's capital counts
AGAINST quorum only while it stays in the pool. The anti-stall property: a large abstainer cannot
indefinitely block finalization — they must either vote or exit, and EXITING shrinks outstanding
and hands the decision to those who stay. Proven end-to-end with a real withdrawal that flips the
trigger: alice (400k = 40%) votes, bob (600k = 60%) abstains -> trigger REJECTED (no quorum); bob
(a non-voter, no vote-lock) withdraws his full 600k principal -> outstanding shrinks to 400k ->
alice is now 100% of the remainder -> trigger SEALS. Distinct from the static minority-turnout
probe (here the abstainer actually leaves). Confirms exits dynamically recompute quorum against
the live pool. Test: twap-program/tests/chain.rs `e2e_non_voter_exit_recomputes_quorum_stayers_decide`.
KEPT.

### [BLOCKED] E2E probe: a non-creator cannot append entries to another's proposal (no injection)
A distribution proposal is built by its CREATOR. append_entries pins header.creator == signer, so
an attacker cannot graft entries (e.g. a self-allocation) onto someone ELSE's proposal — only the
creator can append to it. Otherwise an attacker could inject a payout to themselves onto a popular
proposal before it is voted/sealed. The program-side gate (distribution/src/lib.rs:397) was only
exercised on the positive path (creator appends); the non-creator REJECTION was untested. Pinned
end-to-end: a creator creates a proposal, an attacker's append of a self-allocation is REJECTED,
and the creator's own append succeeds. Complements finding M2 (creator-gated register). Test:
twap-program/tests/chain.rs `e2e_non_creator_cannot_append_to_a_proposal`. KEPT.

### [BLOCKED] E2E probe: no new proposal after the genesis finalizes (one-shot)
The genesis is winner-take-all and one-shot. Once the winning distribution is sealed,
distribution create_proposal rejects (config.is_sealed()), so no NEW proposal can be created on
the finalized config — preventing post-decision clutter or attempts to re-contest a closed
genesis. Proven end-to-end against the real binaries: a voter backs a proposal to quorum +
majority and the trigger seals it; a subsequent create_proposal for a fresh id is REJECTED, and
the sealed winner is unchanged. Complements the seal-irreversibility (finding R / seal.rs) — that
blocks re-sealing, this blocks creating new contenders after the outcome is decided. Test:
twap-program/tests/chain.rs `e2e_no_new_proposal_after_genesis_finalizes`. KEPT.

### [STATE] Coverage map update — distinct attack surface exhausted (33 e2e chain tests)
Full repo green (~107 tests). The e2e harness (twap-program/tests/chain.rs, all six real binaries)
now has 33 tests; every distinct attack class from the probe brief is covered, most from several
angles. Since the last [STATE] entry (at 17 chain tests) the loop added:
 - finding O FIXED (surplus floor via direct slab read + canary) and finding S FIXED (post-handoff
   deposit revoke) — the two real LOFs.
 - handoff/floor: floor-cannot-be-lowered-out-of-band, no-pull-before-floor (fail-safe MAX),
   repeated-pull-cannot-cross-floor, foreign-vault-authority rejected (no cross-market drain),
   cranker-cannot-redirect-surplus, live-floor tracks impairment+recovery, completed-execute
   cannot-be-replayed.
 - liveness: post-handoff exit blocked but DAO-recoverable.
 - governance/economics: anti-flash-vote, no-vote-without-a-position, one-vote-one-proposal,
   minority-turnout / exact-50%-quorum (strict >), 50/50-majority-tie deadlock, non-voter-exit
   recomputes quorum, capital-dominates-log-time (no early squatter), Sybil-split gives no
   advantage, retract/re-back cannot inflate weight.
 - distribution: winner-take-all at claim, no-new-proposal after finalize, non-creator cannot
   append (no injection).
ASSESSMENT: the high-value external-attack surface on the BUILT code is exhaustively covered.
Further ad-hoc probes would be marginal/redundant (reinit guards, capacity bounds, claim-window
edges are already covered by the per-program suites). The right next targets are NOT more probes:
 (1) the unbuilt COIN buy/burn settlement slice (probe once built), and (2) finding L (impairment
 first-come vs pro-rata) which is an open DESIGN decision, not a code bug. Recommend pausing or
 redirecting the 5-minute loop until there is new code to attack.

### [ANALYZED/BLOCKED] deposits_only-policy handoff: the floor protects principal regardless of mode
Investigated a subtle handoff mis-orchestration: the policy rotation to surplus-mode is a SEPARATE
Squads step; what if it is skipped and the twap pulls while the policy is still the genesis
deposits_only (principal-recovery) mode? Read percolator handle_withdraw_insurance_limited
(v16_program.rs ~8542-8558): the withdraw cap is min(max_bps*insurance/1e4, deposit_remaining)
under deposits_only, then bounded by insurance/vault. CONCLUSION: no LOF. The twap's reserved_floor
is an INDEPENDENT, tighter guard applied twap-side BEFORE the CPI — pull_surplus caps `amount` to
insurance - reserved_floor. So the depositor principal sitting in the vault (>= reserved_floor) is
protected whether the policy is deposits_only or surplus-mode; a permissionless cranker can never
pull below the floor in either mode. The policy rotation to surplus-mode is therefore a FUNCTIONAL
requirement (deposits_only caps the operator to deposit_remaining, which is exhausted/zeroed as
depositors exit, so the twap could not pull genuine profits under it), NOT a safety requirement.
Worst case of skipping it: the twap simply cannot pull surplus (functional failure), or percolator's
deposit_remaining accounting drifts under deposits_only — but the actual principal is never lost
(floor-protected) and is recoverable via the DAO re-grant (see the exit-recovery probe). No code
change: the floor (finding O fix, already pinned by e2e_finding_o_floor_blocks_principal_drain and
e2e_twap_resumes_pulling_after_insurance_recovers) is the binding guard and is policy-mode-independent.
