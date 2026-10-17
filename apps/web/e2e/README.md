# Browser smoke tests

A few Playwright flows in a real Chromium, against the real server and a
throwaway PostgreSQL database. They're here to catch what parallel PRs break
between them (sign-in, the task board, a page that no longer renders), not to
be a test matrix. Unit and component tests stay in `src/` with Vitest.

## What's covered

- `smoke.spec.ts`
  - signing in with a password lands on Home with the seeded Project in the rail
  - a wrong password shows an error and stays on the sign-in page
  - creating a Task from the Work items page (it opens in the task panel),
    then moving it on the board: Backlog → Ready with the keyboard (Space,
    an arrow, Space), Ready → In Progress by dragging, and checking it's still
    there after a reload
- `screenshots.spec.ts`: Home, a Project channel, the work item list and
  board, Appearance, Webhooks, API reference and Automation rules settings, each in light and dark ("Match system" with
  the browser's color scheme). Each page must render without uncaught errors
  and resolve to the expected theme. The PNGs land in
  `e2e-output/screenshots/` as artifacts; they're never compared against
  committed images.

## Running locally

You need the server binary, a PostgreSQL you can create databases on, and a
Chromium Playwright can launch.

```bash
cargo build -p agent-workspace-server
export E2E_DATABASE_BASE=postgres://user:password@127.0.0.1:5432   # no database name
corepack pnpm test:e2e                                               # from the repo root
```

If Playwright's own Chromium isn't installed, either install it
(`corepack pnpm --filter @agent-workspace/web exec playwright install chromium`)
or point at one you have: `PLAYWRIGHT_CHROMIUM_EXECUTABLE=/path/to/chrome`.

Other knobs: `E2E_SERVER_BINARY` (default `target/debug/agent-workspace-server`),
`E2E_DATABASE_NAME` (default `aw_e2e`; must start with `aw_e2e`),
`E2E_SERVER_PORT` (default 3190; the webhook listener takes the next port) and
`E2E_WEB_PORT` (default 5190). The defaults stay clear of the usual dev ports,
so a running `pnpm dev` is left alone.

Results, traces of failed tests, the HTML report and `server.log` go to
`apps/web/e2e-output/` (gitignored). `npx playwright show-report e2e-output/report`
from `apps/web` opens the report.

## How it's set up

`global-setup.ts`:

1. drops and recreates the `aw_e2e` database;
2. creates the first User with the real `bootstrap-admin` command.
   It only takes its password from a terminal, so `bootstrap-admin.py` answers
   the prompts on a pseudo-terminal; the password is random per run and only
   ever passes through stdin;
3. starts the server from an empty temp directory (so no `.env` leaks in) in
   `development` mode on its own ports, with only the variables it needs;
4. through the public API, signs in, creates a Project with a `#general`
   channel, a message and a work item, and saves the signed-in session for
   the screenshot spec.

Playwright starts Vite's dev server itself, with
`AGENT_WORKSPACE_API_PROXY_TARGET` pointing its `/api` proxy at that server.
On teardown the server stops and the database is dropped.

In CI, `tools/ci/checks.sh` runs this as the last web step ("web browser
smoke"). It builds the server from the tree first, and skips with a note when
PostgreSQL or a Chromium for Playwright isn't available on the machine.

## TODO

Flows from the roadmap item that wait for other work to land:

- [ ] Start a run on the demo adapter from a work item and watch it finish
      (needs the demo adapter; the server must then run with
      `AGENT_WORKSPACE_DEMO_EXECUTOR=true`, which is development-only, as here).
- [ ] Read the inbox: something addressed to the seeded User shows up, opens,
      and is marked read.
- [ ] Add the inbox and a run page to the light/dark screenshots.
