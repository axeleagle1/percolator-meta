# Security analysis log (adversarial LOF/DOS sweep)

Running note so the 5-min loop doesn't repeat vectors. Format: vector → verdict.

## Analyzed

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
