import { spawn, spawnSync, type ChildProcess } from 'node:child_process'
import { randomBytes, randomUUID } from 'node:crypto'
import { createWriteStream, existsSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'

import { request, type APIRequestContext } from '@playwright/test'

import {
  outputRoot,
  ports,
  seeded,
  serverBinary,
  serverOrigin,
  storageStatePath,
  webOrigin,
} from './environment'

/**
 * Prepares a fresh database, creates the first User through the real
 * `bootstrap-admin` command, starts the built server, and seeds a Project with
 * a channel through the public API. Returns the teardown.
 */
export default async function globalSetup() {
  const databaseBase = (process.env.E2E_DATABASE_BASE ?? process.env.CI_DATABASE_BASE ?? '').replace(/\/+$/, '')
  if (!databaseBase) {
    throw new Error(
      'Set E2E_DATABASE_BASE to a PostgreSQL URL without a database, e.g. postgres://user:pass@127.0.0.1:5432',
    )
  }
  if (!existsSync(serverBinary)) {
    throw new Error(
      `No server binary at ${serverBinary}. Run \`cargo build -p agent-workspace-server\` or set E2E_SERVER_BINARY.`,
    )
  }
  // A server left running by an earlier run would answer the health check and
  // every request after it, and turn down this run's password.
  if (await answers(`${serverOrigin}/health/ready`)) {
    throw new Error(
      `A server is already listening at ${serverOrigin}, probably left running by an earlier run. ` +
        'Stop it, or set E2E_SERVER_PORT to another port.',
    )
  }
  const databaseName = process.env.E2E_DATABASE_NAME ?? 'aw_e2e'
  if (!/^aw_e2e[a-z0-9_]*$/.test(databaseName)) {
    throw new Error('E2E_DATABASE_NAME must start with aw_e2e and use only a-z, 0-9 and _')
  }

  mkdirSync(outputRoot, { recursive: true })
  const runtimeRoot = mkdtempSync(path.join(os.tmpdir(), 'aw-e2e-'))
  const maintenance = `${databaseBase}/postgres`
  const dropDatabase = () =>
    run('dropdb', ['--force', '--if-exists', `--maintenance-db=${maintenance}`, databaseName])
  dropDatabase()
  run('createdb', [`--maintenance-db=${maintenance}`, databaseName])

  // Only these variables reach the server: CI exports AGENT_WORKSPACE_ENV=test
  // and its own database URL, and a developer's .env must not leak in either
  // (the server runs from an empty directory, so dotenv finds nothing).
  const serverEnvironment: NodeJS.ProcessEnv = {
    PATH: process.env.PATH,
    HOME: process.env.HOME,
    AGENT_WORKSPACE_ENV: 'development',
    AGENT_WORKSPACE_DATABASE_URL: `${databaseBase}/${databaseName}`,
    AGENT_WORKSPACE_SERVER_ADDR: `127.0.0.1:${ports.server}`,
    AGENT_WORKSPACE_WEBHOOK_ADDR: `127.0.0.1:${ports.webhook}`,
    AGENT_WORKSPACE_ALLOWED_ORIGINS: webOrigin,
    AGENT_WORKSPACE_PUBLIC_ORIGIN: webOrigin,
    AGENT_WORKSPACE_DATA_ROOT: path.join(runtimeRoot, 'data'),
    AGENT_WORKSPACE_WEB_ROOT: path.join(runtimeRoot, 'web'),
    AGENT_WORKSPACE_CODEX_HOME_ROOT: path.join(runtimeRoot, 'codex'),
    AGENT_WORKSPACE_MINIMUM_FREE_BYTES: String(1024 * 1024 * 1024),
    AGENT_WORKSPACE_LOG_FILTER: 'agent_workspace=info,tower_http=warn',
  }

  // A fresh throwaway password per run, handed over on stdin only.
  const password = `e2e-${randomBytes(18).toString('base64url')}`
  const bootstrap = spawnSync(
    'python3',
    [
      path.join(import.meta.dirname, 'bootstrap-admin.py'),
      serverBinary,
      'bootstrap-admin',
      '--login-name',
      seeded.loginName,
      '--display-name',
      seeded.displayName,
      '--organization-name',
      'Smoke Org',
      '--organization-slug',
      'smoke-org',
    ],
    { cwd: runtimeRoot, env: serverEnvironment, input: `${password}\n`, encoding: 'utf8', timeout: 180_000 },
  )
  if (bootstrap.status !== 0) {
    dropDatabase()
    throw new Error(`bootstrap-admin failed (${bootstrap.status}):\n${bootstrap.stdout}\n${bootstrap.stderr}`)
  }

  const log = createWriteStream(path.join(outputRoot, 'server.log'))
  const server = spawn(serverBinary, [], { cwd: runtimeRoot, env: serverEnvironment, stdio: ['ignore', 'pipe', 'pipe'] })
  server.stdout.pipe(log)
  server.stderr.pipe(log)

  const teardown = async () => {
    await stop(server)
    log.end()
    dropDatabase()
    rmSync(runtimeRoot, { recursive: true, force: true })
  }

  try {
    await waitForHealth(server)
    const seededIds = await seed(password)
    process.env.E2E_PASSWORD = password
    process.env.E2E_PROJECT_ID = seededIds.projectId
    process.env.E2E_CHANNEL_ID = seededIds.channelId
  } catch (error) {
    await teardown()
    throw error
  }
  return teardown
}

function run(command: string, args: string[]) {
  const result = spawnSync(command, args, { encoding: 'utf8' })
  if (result.status !== 0) {
    // stderr from the PostgreSQL tools never includes the password.
    throw new Error(`${command} ${args.at(-1)} failed: ${result.error?.message ?? result.stderr}`)
  }
}

async function answers(url: string) {
  try {
    await fetch(url, { signal: AbortSignal.timeout(2_000) })
    return true
  } catch {
    return false
  }
}

async function waitForHealth(server: ChildProcess) {
  const deadline = Date.now() + 120_000
  while (Date.now() < deadline) {
    if (server.exitCode !== null) {
      throw new Error(`The server exited with ${server.exitCode}; see e2e-output/server.log`)
    }
    try {
      if ((await fetch(`${serverOrigin}/health/ready`)).ok) return
    } catch {
      // not listening yet
    }
    await new Promise((resolve) => setTimeout(resolve, 250))
  }
  throw new Error('The server did not become healthy in time; see e2e-output/server.log')
}

async function stop(server: ChildProcess) {
  if (server.exitCode !== null) return
  const exited = new Promise((resolve) => server.once('exit', resolve))
  server.kill('SIGTERM')
  const timer = setTimeout(() => server.kill('SIGKILL'), 10_000)
  await exited
  clearTimeout(timer)
}

/** Signs in through the API, saves that session for the screenshot spec, and creates the Project. */
async function seed(password: string) {
  const api = await request.newContext({ baseURL: serverOrigin, extraHTTPHeaders: { origin: webOrigin } })
  try {
    await expectOk(api.post('/api/v1/auth/login', { data: { loginName: seeded.loginName, password } }), 'sign in')
    const session = await expectOk(api.get('/api/v1/auth/session'), 'read the session')
    const organizationId = session.organizations?.[0]?.id
    if (!organizationId) throw new Error('The bootstrapped User has no organization')

    const write = async (url: string, data: unknown, what: string) => {
      const state = await api.storageState()
      const csrf = state.cookies.find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
      return expectOk(
        api.post(url, { data, headers: { 'x-csrf-token': csrf, 'idempotency-key': randomUUID() } }),
        what,
      )
    }
    const project = await write(
      '/api/v1/projects',
      { organizationId, key: seeded.projectKey, name: seeded.projectName },
      'create the Project',
    )
    const channel = await write(`/api/v1/projects/${project.id}/channels`, { name: seeded.channelName }, 'create a channel')
    const channelId = channel.conversation?.id ?? channel.channel?.id ?? channel.id
    if (!project.id || !channelId) throw new Error('Project or channel creation returned no id')
    await write(
      `/api/v1/channels/${channelId}/messages`,
      { content: { version: 1, nodes: [{ type: 'text', text: 'Hello from the smoke tests.' }] } },
      'post a message',
    )
    // So the list and board screenshots show a card, whichever spec runs first.
    await write(
      `/api/v1/projects/${project.id}/work-items`,
      { title: 'Seeded work item', description: '', priority: 'medium' },
      'create a work item',
    )

    mkdirSync(path.dirname(storageStatePath), { recursive: true })
    await api.storageState({ path: storageStatePath })
    return { projectId: project.id, channelId }
  } finally {
    await api.dispose()
  }
}

/** The few response fields the seeding reads. */
type Wire = { id?: string; organizations?: Array<{ id?: string }>; conversation?: { id?: string }; channel?: { id?: string } }

async function expectOk(response: ReturnType<APIRequestContext['get']>, what: string): Promise<Wire> {
  const resolved = await response
  if (!resolved.ok()) {
    throw new Error(`Could not ${what}: HTTP ${resolved.status()} ${await resolved.text()}`)
  }
  return (await resolved.json()) as Wire
}
