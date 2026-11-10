# Agent Workspace handoff (2026-11-10)

Development paused here at Mischa's request. **Nothing in this handoff has
been merged or deployed.** `main` is still `9227e49`; the live server runs
what it ran before. AWD-53 (live docs) has only its first, read-only slice,
and the public release ("Path to public") hasn't started.

Repository: <https://github.com/Mischa-dev/agent-workspace> (private).

## The branches

All were pushed as plain branch refs, with their original commits. Pushing a
branch doesn't start CI here (`ci.yml` runs on pull requests and on pushes to
`main` only), and no pull request was opened for any of them.

| Branch | Head | What it is | Validated |
|---|---|---|---|
| [`claude/rmcp-3.9-advisories`](https://github.com/Mischa-dev/agent-workspace/tree/claude/rmcp-3.9-advisories) | [`5e64414`](https://github.com/Mischa-dev/agent-workspace/commit/5e64414d8e0a6bf0c17ec0c79796f5f85dbfc5d2) | Security fix: rmcp 3.0.1 to 3.9.0 | Full run, all checks passed (one flaky retry, below) |
| [`claude/awd-53-doc-blocks`](https://github.com/Mischa-dev/agent-workspace/tree/claude/awd-53-doc-blocks) | [`57d73ef`](https://github.com/Mischa-dev/agent-workspace/commit/57d73efc5aa292e480ee7f610aa511234bf6d7cb) | AWD-53 slice 1: pages read as blocks | Only together with the rmcp fix (next row) |
| [`claude/awd-53-rmcp-validated`](https://github.com/Mischa-dev/agent-workspace/tree/claude/awd-53-rmcp-validated) | [`b7d4caa`](https://github.com/Mischa-dev/agent-workspace/commit/b7d4caac49f3a1f1e6cb314caaa648a7a49c7bb9) | The two above merged (no other change) | Full run, all checks passed |
| [`claude/awd-102-move-resolution`](https://github.com/Mischa-dev/agent-workspace/tree/claude/awd-102-move-resolution) | [`bac63e6`](https://github.com/Mischa-dev/agent-workspace/commit/bac63e655485584c3cbfd1d3d8d355c9f17859cf) | AWD-102: move a task to another project | Full run passed on 2026-11-08 (before the rmcp advisories) |

`claude/awd-53-rmcp-validated` was the local branch
`local/awd-53-rmcp-validate`; its merge commit's message still says "local
only" from before it was published.

### Where to continue

- **Docs work (AWD-53 and on):** start a new branch from
  `claude/awd-53-rmcp-validated` (`b7d4caa`). Don't commit to the four
  branches above; they're kept as reviewed. Once the rmcp fix is on `main`,
  a branch from `main` that merges `claude/awd-53-doc-blocks` is the same.
- **Order to land them:** the rmcp fix first, since `main` and every branch
  now fail `cargo deny` without it; then AWD-102 and AWD-53, each after
  merging the new `main` and passing CI again.

## Validation results

All through the guarded Red Hat route
(`tools/ci/run.sh --host redhat --areas rust,web`), one job at a time.

**Combined head `b7d4caa`** (run `local1794348095_375736`, 2026-11-10, exit
0, "All checks passed (rust,web)", 2,278 s): fmt, web lint and typecheck,
clippy, build; Rust tests **899/899** (2 slow, 0 flaky, no retries); doc
tests; `cargo deny` (advisories, bans, licenses, sources ok); web unit tests
**105 files, 621/621**, including `blockParity.test.ts` (37 tests, all the
shared cases against the real web parser); web build; asset budgets with no
violations (initial JavaScript 433,999 raw / 140,955 gzip bytes, the same as
`main`); performance tools; lockfile policy; browser smoke **97/97**.

**rmcp fix `5e64414`** (run `local1794345527_362488`, exit 0): Rust **891/891
passed, 1 flaky**: `mcp_code::people_start_and_follow_runs_and_answer_approvals`
failed its first try (14.1 s) and passed on the configured retry (18.2 s).
Its first-try output wasn't kept (nextest prints none for a test that passes
on retry), so **its cause is unknown**. It passed first time in the combined
run and in three earlier runs, including one with rmcp 3.9.0. If it fails
again, capture its output before changing anything.

**AWD-102 `bac63e6`** (run `local1794095171_3862990`, 2026-11-08): Rust
905/905, web unit 600/600, browser 98/98, budgets passed after a reviewed
256-byte raw increase. It predates the rmcp advisories, so it now needs the
rmcp fix merged before CI passes again.

## What each change does

### rmcp 3.9.0 (security)

rmcp 3.0.1 has RUSTSEC-2026-0461 (the MCP server transport cached tool
lookups by any requested name without limit, a memory exhaustion risk this
server was exposed to), plus RUSTSEC-2026-0457 and -0462 (client-side, not
used here but failing `cargo deny`). 3.9.0 fixes all three.

- Both exact pins move to `=3.9.0`. The lock adds only what rmcp needs.
- `get_info` returns `ServerConfig` (the same type as the now-deprecated
  `ServerInfo`).
- rmcp 3.9 builds output schemas from what serde writes. Three batch-result
  fields that are always written (`idempotentReplay` twice, `changedFields`)
  are now required in those schemas; the test says so and checks the JSON.
  No response changed.

### AWD-53 slice 1: pages read as blocks (read-only)

`apps/server/src/page_blocks.rs`:

- Splits a page's Markdown into the top-level blocks the web app draws
  (heading, paragraph, code, quote, list, rule, table, and text past the
  100,000-unit parse limit), using the web parser's own top-level rules,
  including JavaScript's character rules where it depends on them.
- Every block is a byte range of the stored text, so nothing is lost or
  rewritten. Outline, section and block-range reads return exact text.
- A block's `position` holds only within one page version; reads by
  position are refused against another version. `hash` checks content, not
  identity. There are **no lasting block ids** yet.
- `blockParity.json` holds 36 cases that both parsers are tested on.

It does **not** yet: expose any route or MCP tool, store documents
differently, edit (no `doc_edit`), suggest, comment, embed, convert existing
pages, or change `page_get` and `page_write` (both work exactly as before).

### AWD-102: move a task to another project

Web review screen and MCP preview/resolution: choose a destination the
server offers, a private preview of what carries over, a choice per issue
(or leave all unresolved), one confirmation. Originals are archived with
every value; omitted private values never reach the destination; links
within the moved tree are remapped. A move is one transaction that rechecks
access and the preview, is idempotent per actor and request, and keeps a
receipt for as long as the original task. Recovery in the browser is per tab
and in memory: a reload forgets an unclear move, and the task itself shows
where it went.

## Other pull requests

- **#205** (resolver over-binding fix): open, not draft, head `5b4f09f`. Its
  CI failed before any test because the laptop's build lock was missing after
  a reboot (since restored). Not rerun or edited. It will also need the rmcp
  fix to pass `cargo deny`.
- **#201** (AWD-85 round 1 directions): stays a draft. Don't merge; it waits
  for Mischa to pick a direction.

## Decisions waiting

1. **Publishing:** open pull requests for the rmcp fix, then AWD-102 and
   AWD-53 (each needs approval; then review, merge and deploy each need
   their own).
2. **AWD-53 before its next slice:** the editor and sync libraries (Tiptap
   with Yjs and `yrs` recommended) and their new dependencies; where a
   document's truth lives (a stored `yrs` document, with Markdown derived);
   what counts as "a person's document", where an agent's edits arrive as
   suggestions by default, and whether a `page_write` to one becomes a
   suggestion or stays a direct write; how existing pages convert; and its
   acceptance criteria (AWD-53 has none yet). AWD-52's other questions are
   answered by the build-out plan page.
3. **Held local work** (preserved, not published): the AWD-102 test-filter
   helper (`local/awd-102-validate`, `local/awd-102-ci-filter`; local-only by
   decision), three `hold/...` branches with AWD-75 webhook and member-list
   commits, and uncommitted CI-guard experiments in an old local worktree
   (not validated, not to be activated).

## The roadmap from here

Build-out order (the "Plan: the build-out phase" page in the AWD project):
AWD-53 docs (next: the stored document and agent block tools, then the
editor, suggestions and comments, the generic embed block, import and
export) → AWD-90 collections, whose views embed through AWD-53's embed block
and use AWD-89 field types → AWD-91 agent parity, which can only pass once
docs and collections exist. Alongside: the UI hill climb (AWD-84 in progress,
AWD-85 waiting on Mischa), and AWD-19 off-site backups and AWD-99 Node on the
Red Hat host, which change the live host and need approval.

## Running the checks

- `tools/ci/run.sh --host redhat --areas rust,web`: the full check, about
  40 minutes on the Red Hat host, one job at a time (a lock in its work
  directory), capped at 10 GiB, each run with its own throwaway database.
  `--areas rust` or `--areas web` runs one side.
- Without `--host`, `run.sh` prefers Mischa's laptop when it's idle. That
  route needs the laptop's build lock file, which a reboot removes and only
  Mischa can restore (it asks for an admin password).
- Agents' local builds on the laptop go through `build-run`, which refuses
  unless the agents' shared memory cap is at most 6 GiB.
- Never run two heavy jobs at once, and never bypass a guard that refuses.
