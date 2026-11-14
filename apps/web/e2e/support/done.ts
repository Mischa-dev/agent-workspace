import path from 'node:path'

import { expect, type APIRequestContext, type BrowserContext, type Page } from '@playwright/test'

import { screenshotDir } from '../environment'

// W1.35: helpers for the test for "done" (END-STATE section 1), one spec per
// item in e2e/done/. Each spec is written by the workstream that builds its
// feature; until then it's a `test.fixme` whose `owner` annotation names
// that workstream (owners-reporter.ts lists them after a run).

/** The signed-in browser's request context, with the headers a write needs. */
export async function writer(context: BrowserContext, page: Page) {
  const csrf = (await context.cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
  const headers = (key?: string) => ({
    'x-csrf-token': csrf,
    origin: new URL(page.url()).origin,
    ...(key ? { 'idempotency-key': key } : {}),
  })
  return { request: page.request, headers }
}

/** The signed-in person's first workspace. */
export async function workspaceId(request: APIRequestContext): Promise<string> {
  const session = (await (await request.get('/api/v1/auth/session')).json()) as { organizations?: Array<{ id: string }> }
  const id = session.organizations?.[0]?.id
  if (!id) throw new Error('The signed-in person has no workspace')
  return id
}

type ModuleView = { id: string; switchable: boolean; switchedOn: boolean }

/**
 * Switch modules in the signed-in person's workspace. `restore()` puts back
 * the switches as they were, so the specs after this one see the workspace
 * they expect.
 */
export async function setModules(context: BrowserContext, page: Page) {
  const { request, headers } = await writer(context, page)
  const organization = await workspaceId(request)
  const path = `/api/v1/organizations/${organization}/modules`
  const before = (await (await request.get(path)).json()) as { modules: ModuleView[] }
  const original = Object.fromEntries(before.modules.filter((module) => module.switchable).map((module) => [module.id, module.switchedOn]))
  const put = async (switches: Record<string, boolean>) => {
    const response = await request.put(path, { headers: headers(), data: { switches } })
    expect(response.ok(), await response.text()).toBe(true)
  }
  return {
    /** Only these modules on (besides the workspace itself). */
    only: (on: string[]) => put(Object.fromEntries(Object.keys(original).map((id) => [id, on.includes(id)]))),
    restore: () => put(original),
  }
}

/** Four screenshots of a page: 1440 and 390 px wide, light and dark. */
export async function look(page: Page, address: string, name: string) {
  for (const [width, height] of [
    [1440, 900],
    [390, 844],
  ] as const) {
    for (const scheme of ['light', 'dark'] as const) {
      await page.setViewportSize({ width, height })
      await page.emulateMedia({ colorScheme: scheme })
      await page.goto(address)
      await page.waitForLoadState('networkidle')
      await page.screenshot({ path: path.join(screenshotDir, 'done', `${name}-${width}-${scheme}.png`), fullPage: true })
    }
  }
}

/**
 * An agent's client for the v2 operations (K2): `asAgent(request, token)`
 * calls an operation the way a script or an agent does, with its bearer.
 */
export function asAgent(request: APIRequestContext, token: string) {
  return {
    call: async (operation: string, input: Record<string, unknown>, workspace?: string) => {
      const route = workspace ? `/api/v2/w/${encodeURIComponent(workspace)}/ops/${operation}` : `/api/v2/ops/${operation}`
      const response = await request.post(route, { headers: { authorization: `Bearer ${token}` }, data: input })
      return (await response.json()) as { outcome: string; value?: unknown; message?: string }
    },
  }
}
