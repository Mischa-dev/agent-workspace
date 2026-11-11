# Claude review handoff

Repository: `Mischa-dev/agent-workspace`.

Base: `dc695b5707ef6f74111f92b2e79ef2c884af1b75`, fetched from main on 2026-11-11 before creating the review branch. Branch: `codex/agent-workspace-findings-20261111-01a1c86d`.

The branch combines independently reviewed PR #211 with local follow-up fixes. It is for Claude's final review. It is not approval to merge or deploy. Main, existing PRs, AWD-107, and the paused organizer were left unchanged.

Read [the complete finding ledger](finding-ledger.md), its [machine-readable version](finding-ledger.json), [the independent PR #211 review](PR-211-review.md), [validation](validation.md), and [commit mapping](commit-mapping.md).

## Scope and outcomes

All 191 assigned Part 1 candidates have an entry, preserving their original claim in JSON. The 48 P1 and 143 P2 entries are numbered AW-001 through AW-191 in their original order. Muse's Part 2 uses different numbering.

- 21 fixed on this branch, including 13 covered by reviewed PR #211 and eight additional candidates.
- 111 already fixed on the base.
- 3 invalid, 1 duplicate.
- 12 design decisions retained.
- 43 blocked for independent completion. These remain unresolved implementation or validation work, not rejected findings or completed fixes. The ledger names the inspected code and the missing regression, behavior choice or implementation. This pass does not establish that all 191 historical defects are remediated.

The uploaded Markdown was readable and exactly 262,776 bytes. SHA-256: `8dc8b7e65c4ef8f78cd65c9c84830ff0ff3a5a7f894734e6652b1a511f97b5e2`. Only this repository's assigned findings are included here. The full multi-repository upload is not published.

## What changed locally after PR #211

- Historical task aliases participate in ambiguity detection. IDs are grouped before limiting, and a current-key fallback cannot undo an ambiguous shared lookup. The archived-original fallback still works when a moved target cannot be read.
- Expired but unswept reply claims cannot post final/interim messages or renew through progress. Reply idempotency binds request ID, generation and completion mode.
- Storage accounting skips vanished descendants while still refusing missing roots and unexpected errors. A symlinked data root is refused.
- Public origins retain their validated canonical URL form.
- Bare notes can be saved without scheduling a reminder.
- Mark all read reports expanded-thread failures while marking the conversations it successfully fetched.
- Conversation tools search backward for the last actual message, preserving /later context after worker events.

## Review risks and decisions

OAuth refresh grace is intentionally unchanged. The shared-token-store parallel-session behavior remains tested. The earlier decisions on full agent scope semantics, show-once webhook secrets, concurrent WorkOS checks, exact trusted private origins and retained offline drafts are unchanged too.

The reply idempotency hash becomes stricter. A retry using an old pre-upgrade reply key can conflict once rather than replay under a context that was not hashed. Ordinary message-send hashes remain compatible. New exact reply retries still replay.

The lease fix does not retract a credentialed push already running in a blocking thread if ownership is lost. PR #211's visibility rechecks improve the identified intervals; they do not promise serializability across every membership mutation. The ledger preserves separate remaining gaps such as webhook multi-address fallback, effective module restrictions at webhook delivery, and incomplete pagination.

PR #205's excluded things-resolver binding bug is not included. Do not merge reference-only PR #207. Open PR #208's task-move work was not incorporated. The held UI draft #201 and page-block PRs #209/#210 were untouched.

## Import and review

With the published branch available:

```sh
git fetch origin codex/agent-workspace-findings-20261111-01a1c86d
git switch -c review/codex-findings FETCH_HEAD
git log --oneline dc695b5707ef6f74111f92b2e79ef2c884af1b75..HEAD
git diff --stat dc695b5707ef6f74111f92b2e79ef2c884af1b75..HEAD
```

Main advanced independently during the review; a later read observed `c2b2a769269137252cc2b80937dc049b2e67a6c2`. This branch was not rebased onto that untested change. Validation applies to the recorded base plus this review series; Claude should assess integration with newer main.

Compare the exact published head supplied with the deliverable. Read the ledger's blocked entries before deciding this closes the broader review sweep. Keep PR #211's original commit mapping in mind to avoid applying its fixes twice.

The separate downloadable package also contains an incremental git bundle and a patch series. In a clone that has the base commit:

```sh
git bundle verify /path/to/agent-workspace-review.bundle
git fetch /path/to/agent-workspace-review.bundle codex/agent-workspace-findings-20261111-01a1c86d:review/codex-findings-bundle
```

Alternatively, start at the base in a clean branch and apply `patches/*.patch` in filename order with `git am`. The package records fresh-checkout import verification, final SHAs and checksums. It contains no deployment or merge action.
