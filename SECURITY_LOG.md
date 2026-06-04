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
