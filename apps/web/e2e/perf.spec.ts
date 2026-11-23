import { spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'

import { expect, test, type BrowserContext, type Page } from '@playwright/test'

import { outputRoot, seededValue, staticOrigin, storageStatePath, webOrigin, webRoot } from './environment'
import { startStaticHost } from './support/staticHost'

// W1.33, speed in CI: the built app on a mid-range phone (4x slower CPU and a
// mobile connection, set through the browser's DevTools protocol), against a
// 10,000-row collection and a busy channel. It reads the app's own
// `aw:route-ready` marks (app/routeReady.ts) and long tasks, and compares
// them with the targets in the web plan (3.9) and the last stored baseline.
// It always reports; it fails only when a time is more than 25% over its
// target. Timings need steady hardware, so it runs on the release host only:
//
//   E2E_PERF=1 corepack pnpm exec playwright test e2e/perf.spec.ts --project chromium
//
// E2E_PERF_BASELINE=write stores this run as the new baseline.

test.skip(!process.env.E2E_PERF, 'Speed is measured on the release host (E2E_PERF=1), where timings are steady.')
test.use({ storageState: storageStatePath })

/** Plan 3.9's targets, in milliseconds. */
const TARGETS = {
  coldShell: 2500,
  coldContent: 3500,
  warmContent: 1200,
  routeToContent: 800,
} as const
type Metric = keyof typeof TARGETS
const SLACK = 1.25
const PHONE = { width: 390, height: 844 }
const BASELINE = path.join(webRoot, 'e2e', 'perf-baseline.json')
const REPORT = path.join(outputRoot, 'perf', 'report.json')
const ROWS = 10_000
const MESSAGES = 300

let stop: (() => Promise<void>) | undefined

async function psql(statement: string, variables: Record<string, string>) {
  const args = [seededValue('E2E_DATABASE_URL'), '-X', '-q', '-t', '-A', '-v', 'ON_ERROR_STOP=1']
  for (const [name, value] of Object.entries(variables)) args.push('-v', `${name}=${value}`)
  const result = spawnSync('psql', args, { input: statement, encoding: 'utf8' })
  if (result.status !== 0) throw new Error(result.stderr || result.error?.message || 'psql failed')
  return result.stdout.trim()
}

async function writer(page: Page) {
  const csrf = (await page.context().cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
  return (key?: string) => ({ 'x-csrf-token': csrf, origin: webOrigin, ...(key ? { 'idempotency-key': key } : {}) })
}

/** A collection of ROWS records and a channel of MESSAGES messages, made once per database. */
async function seed(page: Page) {
  const projectId = seededValue('E2E_PROJECT_ID')
  const channelId = seededValue('E2E_CHANNEL_ID')
  await page.goto(`${webOrigin}/projects/${projectId}/collections`)
  const headers = await writer(page)
  const base = `${webOrigin}/api/v1/projects/${projectId}/collections`
  const listed = (await (await page.request.get(base)).json()) as { collections: Array<{ id: string; name: string }> }
  let collectionId = listed.collections.find((collection) => collection.name === 'Speed rows')?.id
  if (!collectionId) {
    const made = await page.request.post(`${webOrigin}/api/v2/ops/collection_create`, {
      headers: headers('perf-collection'),
      data: { projectId, name: 'Speed rows', titleName: 'Row' },
    })
    expect(made.ok(), await made.text()).toBe(true)
    collectionId = ((await made.json()) as { value: { collection: { id: string } } }).value.collection.id
    const first = await page.request.post(`${base}/${collectionId}/records`, { headers: headers('perf-row-0'), data: { title: 'Row 0' } })
    expect(first.ok(), await first.text()).toBe(true)
    // The rest straight into the table, copied from the first: through the API
    // this would be ten thousand writes before the test even starts.
    await psql(
      `INSERT INTO collection_records (id, project_id, collection_id, number, title, created_by_actor_id, updated_by_actor_id)
       SELECT gen_random_uuid(), r.project_id, r.collection_id, r.number + n, 'Row ' || n, r.created_by_actor_id, r.created_by_actor_id
       FROM collection_records r, generate_series(1, :rows) AS n
       WHERE r.collection_id = :'collection' AND r.number = (SELECT min(number) FROM collection_records WHERE collection_id = :'collection');`,
      { rows: String(ROWS - 1), collection: collectionId },
    )
    for (let index = 0; index < MESSAGES; index += 1) {
      for (let attempt = 0; attempt < 5; attempt += 1) {
        const sent = await page.request.post(`${webOrigin}/api/v2/ops/message_send`, {
          headers: headers(`perf-message-${index}`),
          data: { conversationId: channelId, text: `Status update ${index}: the build is green and the notes are in the doc.` },
        })
        if (sent.status() !== 429) {
          expect(sent.ok(), await sent.text()).toBe(true)
          break
        }
        await page.waitForTimeout(Number(sent.headers()['retry-after'] ?? '1') * 1000)
      }
    }
  }
  return { projectId, channelId, collectionId }
}

/** A mid-range phone: 4x slower CPU and a mobile connection, as DevTools' mobile settings. */
async function asPhone(context: BrowserContext, page: Page) {
  const cdp = await context.newCDPSession(page)
  await cdp.send('Emulation.setCPUThrottlingRate', { rate: 4 })
  await cdp.send('Network.enable')
  await cdp.send('Network.emulateNetworkConditions', {
    offline: false,
    latency: 150,
    downloadThroughput: (1.6 * 1024 * 1024) / 8,
    uploadThroughput: (750 * 1024) / 8,
  })
}

/** Long tasks, counted from the first script on. */
const watchLongTasks = () => {
  const tasks: number[] = []
  ;(window as unknown as { __awLongTasks: number[] }).__awLongTasks = tasks
  try {
    new PerformanceObserver((list) => list.getEntries().forEach((entry) => tasks.push(entry.duration))).observe({ type: 'longtask', buffered: true })
  } catch {
    // No long-task timing in this browser.
  }
}

/** The time `path`'s page took to show its content, from its `aw:route-ready` mark. */
async function routeReady(page: Page, path: RegExp): Promise<number> {
  const handle = await page.waitForFunction(
    (source) => {
      const pattern = new RegExp(source)
      const ready = performance
        .getEntriesByName('aw:route-ready')
        .map((entry) => (entry as PerformanceMark).detail as { path: string; ms: number })
        .filter((detail) => pattern.test(detail.path))
      return ready.length ? ready[ready.length - 1].ms : null
    },
    path.source,
    { timeout: 60_000, polling: 100 },
  )
  return (await handle.jsonValue()) as number
}

const firstContentfulPaint = (page: Page) =>
  page.evaluate(() => performance.getEntriesByName('first-contentful-paint')[0]?.startTime ?? Number.NaN)

const longTasks = (page: Page) =>
  page.evaluate(() => {
    const tasks = (window as unknown as { __awLongTasks?: number[] }).__awLongTasks ?? []
    return { count: tasks.length, ms: Math.round(tasks.reduce((sum, duration) => sum + duration, 0)) }
  })

test.beforeAll(async () => {
  test.setTimeout(300_000)
  stop = await startStaticHost()
})

test.afterAll(async () => {
  await stop?.()
})

test('the app is quick enough on a mid-range phone', async ({ browser, page }) => {
  test.setTimeout(600_000)
  const { projectId, channelId, collectionId } = await seed(page)
  const measured: Partial<Record<Metric, number>> = {}
  const routes: Record<string, number> = {}

  const context = await browser.newContext({ storageState: storageStatePath, viewport: PHONE, isMobile: true, hasTouch: true })
  await context.addInitScript(watchLongTasks)
  const phone = await context.newPage()
  await asPhone(context, phone)

  // Cold: nothing cached.
  await phone.goto(`${staticOrigin}/inbox`)
  measured.coldContent = await routeReady(phone, /\/inbox$/)
  measured.coldShell = await firstContentfulPaint(phone)

  // Warm: the same browser opens the app again.
  await phone.goto(`${staticOrigin}/inbox`)
  measured.warmContent = await routeReady(phone, /\/inbox$/)

  // Moving inside the app: the 10,000-row collection, then the busy channel.
  await phone.evaluate((href) => {
    history.pushState(null, '', href)
    dispatchEvent(new PopStateEvent('popstate'))
  }, `/projects/${projectId}/collections/${collectionId}`)
  routes.collection = await routeReady(phone, /\/collections\/[^/]+$/)
  await phone.evaluate((href) => {
    history.pushState(null, '', href)
    dispatchEvent(new PopStateEvent('popstate'))
  }, `/projects/${projectId}/channels/${channelId}`)
  routes.channel = await routeReady(phone, /\/(?:channels|chat)\/[^/]+$/)
  measured.routeToContent = Math.max(routes.collection, routes.channel)
  const tasks = await longTasks(phone)
  await context.close()

  const baseline = existsSync(BASELINE) ? (JSON.parse(readFileSync(BASELINE, 'utf8')) as Partial<Record<Metric, number>>) : {}
  const rows = (Object.keys(TARGETS) as Metric[]).map((metric) => {
    const value = Math.round(measured[metric] ?? Number.NaN)
    const before = baseline[metric]
    return {
      metric,
      ms: value,
      target: TARGETS[metric],
      baseline: before ?? null,
      change: before ? `${Math.round(((value - before) / before) * 100)}%` : 'no baseline yet',
      over: value > TARGETS[metric] * SLACK,
    }
  })
  const report = { at: new Date().toISOString(), rows, routes, longTasks: tasks }
  mkdirSync(path.dirname(REPORT), { recursive: true })
  writeFileSync(REPORT, `${JSON.stringify(report, null, 2)}\n`)
  if (process.env.E2E_PERF_BASELINE === 'write') {
    writeFileSync(BASELINE, `${JSON.stringify(Object.fromEntries(rows.map((row) => [row.metric, row.ms])), null, 2)}\n`)
  }
  console.table(rows)
  console.log(`Routes: ${JSON.stringify(routes)}. Long tasks: ${tasks.count}, ${tasks.ms} ms in all.`)
  await test.info().attach('speed', { body: JSON.stringify(report, null, 2), contentType: 'application/json' })

  expect(rows.filter((row) => row.over).map((row) => `${row.metric}: ${row.ms} ms, target ${row.target} ms`)).toEqual([])
})
