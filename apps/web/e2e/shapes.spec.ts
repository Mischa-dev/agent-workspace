import { spawnSync } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { readFileSync } from 'node:fs'
import path from 'node:path'

import { expect, test, type Page } from '@playwright/test'

import { repoRoot, seededValue, serverOrigin, storageStatePath } from './environment'

// W1.39, the shape walk (R22, R31): a workspace made from each shape shows
// nothing of a module that's off, in any words. Each walk opens the shell,
// every settings page, search, the command list and the phone bar, and
// fails on a word that belongs to a module the shape turns off. Workspaces
// can't be made through the API yet, so the walk adds each with SQL (values
// passed as psql variables, never spliced in) and sets its shape through
// the operation the settings page uses.

test.use({ storageState: storageStatePath })

const SHAPES = ['friends', 'team', 'just_me', 'documents', 'blank'] as const

/** The words that belong to a module, as a person reads them anywhere. */
const MODULE_WORDS: Record<string, string[]> = {
  projects: ['project', 'projects'],
  tasks: ['task', 'tasks', 'subtask', 'subtasks', 'backlog', 'milestone', 'milestones'],
  agents: ['agent', 'agents'],
  runs: ['run', 'runs'],
  channels: ['channel', 'channels'],
  whiteboards: ['whiteboard', 'whiteboards'],
  sheets: ['sheet', 'sheets'],
  collections: ['collection', 'collections'],
  automations: ['automation', 'automations'],
  dashboards: ['dashboard', 'dashboards'],
  code: ['repository', 'repositories', 'pull request', 'pull requests'],
}

/**
 * Settings pages that list every module on purpose: where they're switched
 * on (Modules, Templates, Layout), what roles may do in each (Roles), the
 * server's and the audit log's records, and the whole workspace's export and
 * metered usage (an export carries everything; usage reports every meter).
 */
const CONFIGURATION = /\/settings\/(?:spaces\/[^/]+\/)?(modules|templates|project-templates|layout|roles|server|audit-log|harnesses|hosts|github|export|usage)$/

function shapeOff(shape: string): string[] {
  const file = path.join(repoRoot, 'modules/packages/shapes', shape, 'package.json')
  const manifest = JSON.parse(readFileSync(file, 'utf8')) as { contents: { kind: string; body: { off?: string[] } }[] }
  return manifest.contents.find((content) => content.kind === 'switches')?.body.off ?? []
}

function sql(statement: string, variables: Record<string, string>) {
  const args = [seededValue('E2E_DATABASE_URL'), '-X', '-q', '-t', '-A', '-v', 'ON_ERROR_STOP=1']
  for (const [name, value] of Object.entries(variables)) args.push('-v', `${name}=${value}`)
  const result = spawnSync('psql', args, { input: statement, encoding: 'utf8' })
  if (result.status !== 0) throw new Error(result.stderr || result.error?.message || 'psql failed')
  return result.stdout.trim()
}

async function csrfHeaders(page: Page) {
  const csrf = (await page.context().cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
  return { 'x-csrf-token': csrf, origin: new URL(page.url()).origin }
}

/** The words of switched-off modules on screen now, with a little of what's around each. */
async function offWords(page: Page, words: string[]) {
  if (!words.length) return []
  // What's shown, and what a screen reader says: labels and titles too.
  const text = await page.evaluate(() => {
    // Account settings are the person's own, across workspaces: not this shape's.
    const account = document.querySelector<HTMLElement>('[aria-label="Account settings"]')
    if (account) account.style.display = 'none'
    const labels = Array.from(document.querySelectorAll<HTMLElement>('[aria-label], [title]'))
      .filter((element) => !element.closest('[aria-label="Account settings"]'))
      .filter((element) => element.getClientRects().length > 0 || element.closest('nav, [role="group"]'))
      .map((element) => `${element.getAttribute('aria-label') ?? ''} ${element.getAttribute('title') ?? ''}`)
    const text = [document.body.innerText, ...labels].join('\n')
    if (account) account.style.display = ''
    return text
  })
  const found: string[] = []
  for (const word of words) {
    const pattern = new RegExp(`\\b${word.replace(' ', '\\s+')}\\b`, 'gi')
    for (const match of text.matchAll(pattern)) {
      const at = match.index ?? 0
      found.push(`"${word}" in “${text.slice(Math.max(0, at - 30), at + word.length + 30).replace(/\s+/g, ' ').trim()}”`)
    }
  }
  return found
}

for (const shape of SHAPES) {
  test(`a ${shape} workspace shows nothing of a module that's off`, async ({ page }) => {
    test.setTimeout(180_000)
    await page.goto('/w/-/home')
    const session = (await (await page.request.get(`${serverOrigin}/api/v1/auth/session`)).json()) as { user: { actorId: string } }
    const workspaceId = randomUUID()
    const slug = `shape-${shape.replace('_', '-')}-${workspaceId.slice(0, 8)}`
    sql(
      `INSERT INTO organizations (id, name, slug) VALUES (:'id', 'Walk', :'slug');
       INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES (:'id', :'actor', 'owner');`,
      { id: workspaceId, slug, actor: session.user.actorId },
    )
    try {
      const set = await page.request.post(`${serverOrigin}/api/v2/w/${workspaceId}/ops/workspace_shape_set`, {
        headers: { ...(await csrfHeaders(page)), 'content-type': 'application/json', 'idempotency-key': randomUUID() },
        data: { shape },
      })
      expect(set.ok(), await set.text()).toBe(true)

      const words = shapeOff(shape).flatMap((module) => MODULE_WORDS[module] ?? [])
      const found: string[] = []
      const look = async (where: string) => {
        await page.waitForLoadState('networkidle')
        for (const word of await offWords(page, words)) found.push(`${where}: ${word}`)
      }

      // The shell and Home.
      await page.goto(`/w/${slug}/home`)
      await look('home')
      for (const address of ['/inbox', '/messages', '/later', '/search']) {
        await page.goto(`/w/${slug}${address}`)
        await look(address)
      }

      // Every settings page the workspace's settings list, except the ones
      // where you choose and audit what's there, which name every module.
      await page.goto(`/w/${slug}/settings`)
      await look('settings')
      const lists = page.locator('[aria-label="Workspace settings"] a, [aria-label="Project settings"] a, [aria-label="Space settings"] a')
      const links = await lists.evaluateAll((anchors) => anchors.map((anchor) => (anchor as HTMLAnchorElement).pathname))
      const walked = [...new Set(links)].filter((link) => link.startsWith(`/w/${slug}/settings`) && !CONFIGURATION.test(link))
      for (const link of walked) {
        await page.goto(link)
        await look(link)
      }

      // The command list.
      await page.goto(`/w/${slug}/home`)
      await page.waitForLoadState('networkidle')
      await page.getByRole('button', { name: 'Search and commands' }).first().click()
      await expect(page.getByRole('dialog').first()).toBeVisible()
      await look('command list')
      await page.keyboard.press('Escape')

      // The phone bar.
      await page.setViewportSize({ width: 390, height: 844 })
      await page.goto(`/w/${slug}/home`)
      await look('phone')

      expect(found).toEqual([])
    } finally {
      sql(`UPDATE organization_memberships SET enabled = false WHERE organization_id = :'id';`, { id: workspaceId })
    }
  })
}
