import path from 'node:path'
import { fileURLToPath } from 'node:url'

/** Shared by playwright.config.ts, the global setup and the specs. */
const here = path.dirname(fileURLToPath(import.meta.url))

export const webRoot = path.resolve(here, '..')
export const repoRoot = path.resolve(webRoot, '../..')
export const outputRoot = path.join(webRoot, 'e2e-output')
export const screenshotDir = path.join(outputRoot, 'screenshots')
export const storageStatePath = path.join(outputRoot, 'state', 'signed-in.json')

// Ports away from the usual dev ones (3100/5173), so a running dev setup is left alone.
const serverPort = Number(process.env.E2E_SERVER_PORT ?? 3190)
const webPort = Number(process.env.E2E_WEB_PORT ?? 5190)

export const ports = { server: serverPort, webhook: serverPort + 1, web: webPort }
export const serverOrigin = `http://127.0.0.1:${serverPort}`
export const webOrigin = `http://127.0.0.1:${webPort}`

export const serverBinary =
  process.env.E2E_SERVER_BINARY ??
  path.join(process.env.CARGO_TARGET_DIR ?? path.join(repoRoot, 'target'), 'debug', 'agent-workspace-server')

/** Seeded by global-setup.ts; specs read them through these names. */
export const seeded = {
  loginName: 'smoke',
  displayName: 'Smoke Tester',
  projectName: 'Smoke Project',
  projectKey: 'SMK',
  channelName: 'general',
}

export function seededValue(name: 'E2E_PASSWORD' | 'E2E_PROJECT_ID' | 'E2E_CHANNEL_ID'): string {
  const value = process.env[name]
  if (!value) throw new Error(`${name} is missing; the global setup didn't run`)
  return value
}
