---
name: steward
description: Use when driving an Agent Workspace pull request to green, such as before pushing, when CI fails, when the branch conflicts with main, or when answering review comments.
---

# Drive a pull request to green

## Before every push

Run the same checks GitHub runs, on the working tree as it is:

- On the CI machines: `tools/ci/run.sh` (add `--areas rust` or
  `--areas web` for one side). It picks the idle laptop or the Red Hat host.
- In a cloud session, where those hosts aren't reachable, run the checks
  script directly after `tools/cloud/setup.sh`:

  ```bash
  CI_RUN_ID=local CI_DATABASE_BASE=postgres://agent_workspace:agent_workspace@127.0.0.1:5432 \
  PLAYWRIGHT_CHROMIUM_EXECUTABLE=/opt/pw-browsers/chromium tools/ci/checks.sh
  ```

  It needs `rsync` for its own script tests; install it if it's missing.

The checks, in order: CI script tests, `cargo fmt --check`, the self-hosted
guide and fixture checks, web lint and typecheck, clippy with `-D warnings`,
all Rust tests against a throwaway database (`cargo nextest`), doc tests,
`cargo deny`, then web tests, the web build, the asset budgets and the
browser smoke tests. Fix what fails before pushing; one validated push beats
several guesses.

New integration tests build on `apps/server/tests/integration/support/`
(the app, test databases, callers and worlds) rather than a file's private
helpers.

## Conflicts and a moving main

- Several workstreams merge into `main` while you work, and the repository
  requires branches to be up to date. Merge `origin/main` into your branch
  (no rebase, no force-push on a shared branch), resolve, run the checks,
  push.
- GitHub runs no CI on a pull request that conflicts with `main`, so a PR
  with no checks running usually needs a merge.
- Where both sides add to a shared registry (the tool list in `mcp.rs`,
  routes in `api.rs` and `App.tsx`, `policy_inventory.rs`, `ui/index.ts`,
  `MCP_CASE_TOOLS` in the isolation test, `WRITE_ROUTES` and
  `MCP_WRITE_TOOLS` in the write isolation test), keep both sides' lines.
- Two migrations with the same timestamp, or a numbered one: rename yours to
  a new timestamp in your range.
- Regenerate `pnpm-lock.yaml` with `corepack pnpm install`, never by hand.

## When CI is red

- GitHub's CI runs `tools/ci/checks.sh` on a GitHub-hosted `ubuntu-latest`
  runner (`.github/workflows/ci.yml`), never on our own machines; jobs queue
  when several branches push at once. A queued job is not a failure. Locally,
  `tools/ci/run.sh` runs the same checks on the laptop or the Red Hat host.
- Read the failing step's log. Reproduce it locally with the same command
  before changing anything, then show it passing.
- Tests that need PostgreSQL skip without `AGENT_WORKSPACE_DATABASE_URL`;
  locally, a skip is not a pass.
- Never skip, ignore or loosen a test to get green. Never pin a count other
  branches change (tool counts, bundle totals).

## Review comments

Fix small, local asks and push. Answer design questions on the thread with
a proposal. Every change still passes the checks above before it's pushed.

## The pull request

- Title `<slice id>: <name>` (`K1.3: …`), or `Build <n>: <name>` for a
  whole build; one branch and one PR each.
- The description lists deploy requirements: new migrations, environment
  variables, permissions, anything to install or set after deploying. The
  deploy checklist in `docs/operations/alpha-deploy.md` collects them.
- Say in the description which slices of the workstream plan's slice table
  (`docs/plans/master/plans/`, and `SLICES.md`) the PR builds, and record any
  interface it changes in `docs/plans/master/interfaces.md`, in the same PR.
