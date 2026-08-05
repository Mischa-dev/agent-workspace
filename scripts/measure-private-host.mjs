import { spawnSync } from 'node:child_process'
import { lstatSync, readFileSync } from 'node:fs'
import { release } from 'node:os'
import { isAbsolute } from 'node:path'
import { performance } from 'node:perf_hooks'

import {
  MEASURED_SAMPLES,
  WARMUP_SAMPLES,
  parseSetCookieHeaders,
  selectTimelineThread,
  summarizeDurations,
  validateLoopbackBaseUrl,
} from './private-host-performance-lib.mjs'

const MAX_RESPONSE_BYTES = 1024 * 1024
const READ_BUDGET_MS = 250
const WRITE_BUDGET_MS = 500

const options = parseOptions(process.argv.slice(2))
const baseUrl = validateLoopbackBaseUrl(required(options, 'base-url'))
const loginName = required(options, 'login-name')
if (!/^[a-z0-9][a-z0-9._-]{0,63}$/.test(loginName)) fail('login name is invalid')
const passwordPath = required(options, 'password-file')
const password = readOwnerOnlyPassword(passwordPath)
const git = spawnSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] })
const commit = git.status === 0 ? git.stdout.trim() : 'unknown'

let cookieHeader = ''
let csrfToken = ''
try {
  const login = await boundedFetch('/api/v1/auth/login', {
    method: 'POST',
    json: { loginName, password },
    authenticated: false,
  })
  if (login.status !== 200 || login.json?.authenticated !== true) fail('host login was not accepted')
  const cookies = parseSetCookieHeaders(login.headers)
  cookieHeader = [...cookies].map(([name, value]) => `${name}=${value}`).join('; ')
  csrfToken = cookies.get('agent_workspace_csrf') ?? ''
  if (!cookieHeader || !csrfToken) fail('host login did not return the required bounded session cookies')

  const projectsResponse = await boundedFetch('/api/v1/projects?limit=100')
  const projects = Array.isArray(projectsResponse.json?.projects)
    ? projectsResponse.json.projects
    : Array.isArray(projectsResponse.json) ? projectsResponse.json : []
  const project = projects.find((candidate) => candidate && typeof candidate.id === 'string' && !candidate.archivedAt)
  if (!project) fail('measurement fixture has no active Project')
  const projectId = encodeURIComponent(project.id)
  const initialSidebar = await boundedFetch(`/api/v1/projects/${projectId}/conversation-sidebar`)
  const initialChannels = Array.isArray(initialSidebar.json?.channels) ? initialSidebar.json.channels : []
  const selectedChannel = initialChannels.find((channel) => channel && typeof channel.id === 'string')
  const sidebarResponse = selectedChannel
    ? await boundedFetch(
        `/api/v1/projects/${projectId}/conversation-sidebar?selectedChannelId=${encodeURIComponent(selectedChannel.id)}`,
      )
    : initialSidebar
  const channels = Array.isArray(sidebarResponse.json?.channels) ? sidebarResponse.json.channels : []
  const threads = channels.flatMap((channel) => Array.isArray(channel?.threads) ? channel.threads : [])
  const timelineThread = selectTimelineThread(channels)
  const workItemsResponse = await boundedFetch(`/api/v1/projects/${projectId}/work-items?limit=50`)
  const workItems = Array.isArray(workItemsResponse.json?.items) ? workItemsResponse.json.items : []

  const endpoints = []
  endpoints.push(await measureEndpoint('project_list', '/api/v1/projects?limit=100', READ_BUDGET_MS))
  endpoints.push(await measureEndpoint('project_read', `/api/v1/projects/${projectId}`, READ_BUDGET_MS))
  endpoints.push(await measureEndpoint('conversation_sidebar', `/api/v1/projects/${projectId}/conversation-sidebar`, READ_BUDGET_MS))
  endpoints.push(await measureEndpoint('work_item_list', `/api/v1/projects/${projectId}/work-items?limit=50`, READ_BUDGET_MS))
  if (timelineThread?.id) {
    endpoints.push(await measureEndpoint(
      'thread_timeline',
      `/api/v1/conversations/${encodeURIComponent(timelineThread.id)}/timeline?limit=100`,
      READ_BUDGET_MS,
    ))
  }

  let navigationVersion = Number(project.navigation?.version)
  if (!Number.isSafeInteger(navigationVersion) || navigationVersion < 0) fail('Project navigation version is unavailable')
  endpoints.push(await measureEndpoint(
    'project_switch_write',
    `/api/v1/projects/${projectId}/switch`,
    WRITE_BUDGET_MS,
    () => ({ method: 'POST', json: { expectedVersion: navigationVersion } }),
    (response) => {
      const nextVersion = Number(response.json?.navigation?.version)
      if (!Number.isSafeInteger(nextVersion) || nextVersion <= navigationVersion) fail('write sample did not advance navigation version')
      navigationVersion = nextVersion
    },
  ))

  const report = {
    schemaVersion: 1,
    commit,
    host: { architecture: process.arch, platform: process.platform, kernel: release() },
    target: 'private loopback host',
    warmupSamples: WARMUP_SAMPLES,
    measuredSamples: MEASURED_SAMPLES,
    fixture: {
      projects: projects.length,
      channels: channels.length,
      threads: threads.length,
      workItemsOnFirstPage: workItems.length,
    },
    endpoints,
    failures: endpoints.flatMap((endpoint) => endpoint.passed ? [] : [{ endpoint: endpoint.name, category: 'p95_budget_exceeded' }]),
  }
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`)
  if (report.failures.length) process.exitCode = 1
} catch (error) {
  process.stderr.write(`performance measurement failed: ${safeError(error)}\n`)
  process.exitCode = 1
} finally {
  if (cookieHeader && csrfToken) {
    await boundedFetch('/api/v1/auth/logout', { method: 'POST', json: {} }).catch(() => undefined)
  }
}

async function measureEndpoint(name, path, budgetMs, optionsFactory = () => ({}), inspect = () => {}) {
  for (let index = 0; index < WARMUP_SAMPLES; index += 1) inspect(await boundedFetch(path, optionsFactory()))
  const durations = []
  for (let index = 0; index < MEASURED_SAMPLES; index += 1) {
    const startedAt = performance.now()
    const response = await boundedFetch(path, optionsFactory())
    durations.push(performance.now() - startedAt)
    inspect(response)
  }
  const summary = summarizeDurations(durations)
  return { name, budgetMs, ...summary, passed: summary.p95Ms < budgetMs }
}

async function boundedFetch(path, { method = 'GET', json, authenticated = true } = {}) {
  const headers = { accept: 'application/json', origin: baseUrl }
  if (authenticated) headers.cookie = cookieHeader
  if (json !== undefined) headers['content-type'] = 'application/json'
  if (method !== 'GET' && authenticated) headers['x-csrf-token'] = csrfToken
  const response = await fetch(`${baseUrl}${path}`, {
    method,
    headers,
    body: json === undefined ? undefined : JSON.stringify(json),
    redirect: 'error',
    signal: AbortSignal.timeout(10_000),
  })
  const declaredBytes = Number(response.headers.get('content-length') ?? 0)
  if (Number.isFinite(declaredBytes) && declaredBytes > MAX_RESPONSE_BYTES) fail('response exceeded the measurement cap')
  const bytes = new Uint8Array(await response.arrayBuffer())
  if (bytes.length > MAX_RESPONSE_BYTES) fail('response exceeded the measurement cap')
  let parsed = null
  if (bytes.length) {
    try { parsed = JSON.parse(new TextDecoder().decode(bytes)) } catch { fail('response was not bounded JSON') }
  }
  if (!response.ok) fail(`endpoint returned bounded HTTP ${response.status}`)
  return { status: response.status, headers: response.headers, json: parsed }
}

function readOwnerOnlyPassword(path) {
  if (!isAbsolute(path)) fail('password file path must be absolute')
  const metadata = lstatSync(path)
  if (!metadata.isFile() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0 || metadata.size < 12 || metadata.size > 1025) {
    fail('password file must be an owner-only regular file containing 12-1024 bytes')
  }
  return readFileSync(path, 'utf8').replace(/\r?\n$/, '')
}

function parseOptions(arguments_) {
  const parsed = new Map()
  for (let index = 0; index < arguments_.length; index += 2) {
    const name = arguments_[index]?.match(/^--([a-z-]+)$/)?.[1]
    const value = arguments_[index + 1]
    if (!name || value === undefined || parsed.has(name)) fail('measurement options must be unique --name value pairs')
    parsed.set(name, value)
  }
  for (const name of parsed.keys()) if (!['base-url', 'login-name', 'password-file'].includes(name)) fail('unknown measurement option')
  return parsed
}

function required(values, name) {
  const value = values.get(name)
  if (!value) fail(`--${name} is required`)
  return value
}

function safeError(error) {
  return error instanceof Error ? error.message.slice(0, 240).replace(/[\r\n\x00-\x1f\x7f]/g, ' ') : 'unknown bounded failure'
}

function fail(message) {
  throw new Error(message)
}
