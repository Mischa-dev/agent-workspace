import { spawnSync } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { mkdirSync, writeFileSync } from 'node:fs'
import path from 'node:path'

import { expect, type APIRequestContext, type BrowserContext, type Page } from '@playwright/test'

import { screenshotDir, seeded, seededValue, serverBinary, webOrigin } from '../environment'

// W1.35: helpers for the test for "done" (END-STATE section 1), one spec per
// item in e2e/done/. Each spec is written by the workstream that builds its
// feature; until then it's a `test.fixme` whose `owner` annotation names
// that workstream (owners-reporter.ts lists them after a run).

/** The signed-in browser's request context, with the headers a write needs. */
export async function writer(context: BrowserContext, page: Page) {
  const csrf = (await context.cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
  const headers = (key?: string) => ({
    'x-csrf-token': csrf,
    origin: page.url().startsWith('http') ? new URL(page.url()).origin : webOrigin,
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
    const response = await request.post(`/api/v2/w/${organization}/ops/workspace_update`, {
      headers: headers(randomUUID()),
      data: { projectId: 'workspace', modules: switches },
    })
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
      // v2 refuses a write without an Idempotency-Key (ruling R52): one per call here.
      const headers = { authorization: `Bearer ${token}`, 'idempotency-key': randomUUID() }
      const response = await request.post(route, { headers, data: input })
      return (await response.json()) as { outcome: string; value?: unknown; message?: string }
    },
  }
}

type Session = { organizations?: Array<{ id: string; name: string; slug: string }> }

/** The signed-in person's workspaces, as the session lists them. */
export async function workspaces(request: APIRequestContext) {
  return ((await (await request.get('/api/v1/auth/session')).json()) as Session).organizations ?? []
}

/**
 * Imports an export archive as a new workspace owned by `ownerLogin`, with
 * the server's own command (`workspace-import`), as an operator of another
 * server would. The archive's ids are already here, so it comes in as a copy.
 */
export function importArchive(archive: string, ownerLogin: string, slug: string) {
  const { cwd, ...env } = JSON.parse(seededValue('E2E_SERVER_ENVIRONMENT')) as Record<string, string>
  const result = spawnSync(serverBinary, ['workspace-import', '--file', archive, '--owner-login', ownerLogin, '--slug', slug], {
    cwd,
    env,
    encoding: 'utf8',
    timeout: 120_000,
  })
  if (result.status !== 0) throw new Error(`workspace-import failed (${result.status}): ${result.stdout}\n${result.stderr}`)
}

/** Starts an export of the signed-in person's first workspace and waits for its archive; returns where it was saved. */
export async function exportArchive(context: BrowserContext, page: Page): Promise<string> {
  const { request, headers } = await writer(context, page)
  const workspace = await workspaceId(request)
  const started = await request.post(`/api/v2/w/${workspace}/ops/workspace_export_start`, { headers: headers(randomUUID()), data: {} })
  expect(started.ok(), await started.text()).toBe(true)
  return downloadExport(request, workspace, headers())
}

/** Waits for the newest export's archive and saves it. */
export async function downloadExport(request: APIRequestContext, workspace: string, headers: Record<string, string>): Promise<string> {
  type Transfer = { direction: string; state: string; downloadUrl?: string | null }
  let url = ''
  await expect
    .poll(
      async () => {
        const listed = (await (await request.post(`/api/v2/w/${workspace}/ops/workspace_transfer_list`, { headers, data: {} })).json()) as {
          value?: { transfers: Transfer[] }
          transfers?: Transfer[]
        }
        const newest = (listed.value?.transfers ?? listed.transfers ?? []).find((transfer) => transfer.direction === 'export')
        url = newest?.downloadUrl ?? ''
        return newest?.state
      },
      { timeout: 60_000 },
    )
    .toBe('succeeded')
  const archive = await request.get(url)
  expect(archive.ok()).toBe(true)
  const file = path.join(screenshotDir, '..', `export-${randomUUID()}.tar`)
  mkdirSync(path.dirname(file), { recursive: true })
  writeFileSync(file, await archive.body())
  return file
}

/**
 * Another workspace for the signed-in person (a copy of the first, imported
 * from its export), made once per slug and reused after that.
 */
export async function secondWorkspace(context: BrowserContext, page: Page, slug = 'smoke-copy') {
  const existing = (await workspaces(page.request)).find((workspace) => workspace.slug === slug)
  if (existing) return existing
  importArchive(await exportArchive(context, page), seeded.loginName, slug)
  const made = (await workspaces(page.request)).find((workspace) => workspace.slug === slug)
  if (!made) throw new Error('The imported workspace is not in the session')
  return made
}
