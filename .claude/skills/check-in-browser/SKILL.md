---
name: check-in-browser
description: Use when a change to the Agent Workspace web app needs looking at in a real browser, such as checking a page renders, taking light and dark screenshots, or walking a flow on a phone-sized screen.
---

# Check a change in a real browser

The Playwright setup in `apps/web/e2e` starts the real server built from this
tree against a throwaway PostgreSQL database, seeds a user and a project, and
signs in. Use it rather than starting servers by hand.

## Run it

In a Claude Code or Codex cloud session (after `tools/cloud/setup.sh`):

```bash
cargo build -p agent-workspace-server --bin agent-workspace-server
E2E_DATABASE_BASE=postgres://agent_workspace:agent_workspace@127.0.0.1:5432 \
PLAYWRIGHT_CHROMIUM_EXECUTABLE=/opt/pw-browsers/chromium \
corepack pnpm test:e2e
```

Never run `playwright install` in a cloud session; Chromium is already there.
Elsewhere, `apps/web/e2e/README.md` says what's needed.

## Look at one page

Write a throwaway spec, say `apps/web/e2e/my-check.spec.ts`, that opens the
page and saves a screenshot, then delete it before committing:

```ts
import { expect, test } from '@playwright/test'
import { screenshotDir, seededValue, storageStatePath } from './environment'

test.use({ storageState: storageStatePath })

test('the page renders', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }) // a phone
  await page.goto(`/projects/${seededValue('E2E_PROJECT_ID')}/pages`)
  await expect(page.getByRole('heading', { name: 'Pages' })).toBeVisible()
  await page.screenshot({ path: `${screenshotDir}/my-check.png`, fullPage: true })
})
```

Run it alone with the same variables and
`corepack pnpm --filter @agent-workspace/web exec playwright test my-check`. Read the PNG to look
at it. `screenshots.spec.ts` shows light and dark.

## Look at a slice the way the test for "done" does

`apps/web/e2e/support/done.ts` has `look(page, path, name)`: the page at
1440 and 390 px, light and dark, saved to `e2e-output/screenshots/done/`.
For a live feature, open a second browser context as another person, and
drive an agent with `asAgent(request, token).call(operation, input)` to
watch its change arrive. When your slice finishes an item of the test for
"done", replace its fixme in `e2e/done/items.spec.ts` with a spec of its
own (`e2e/done/NN-<name>.spec.ts`); it runs at a desktop and on a phone.

## What to look for

New pages must pass `docs/frontend/polish-checklist.md`: loading, empty,
error, keyboard and phone states. New pages go in lazy chunks; the initial
load is capped (`pnpm check:assets` runs the budget).
