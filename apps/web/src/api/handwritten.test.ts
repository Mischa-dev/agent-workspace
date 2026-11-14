/// <reference types="node" />
import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { join, relative } from 'node:path'

import { describe, expect, it } from 'vitest'

// The hand-written-call ratchet (W1.4, W1 section 10.1). New calls go
// through the op client (api/ops.ts); each file's hand-written
// `requestJson` calls may only go down. When a file has fewer than its
// allowance, lower it: UPDATE_LEGACY_ALLOWLIST=1 rewrites the list.

const SRC = join(process.cwd(), 'src')
const LIST = join(SRC, 'api/legacy-allowlist.json')
// The request function itself and the generated client are the plumbing.
const PLUMBING = new Set(['api/client.ts', 'api/generated/client.ts'])
const CALL = /\brequestJson\s*(?:<|\()/g

function files(folder: string): string[] {
  return readdirSync(folder).flatMap((name) => {
    const path = join(folder, name)
    if (statSync(path).isDirectory()) return files(path)
    return /\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name) ? [path] : []
  })
}

function counts(): Record<string, number> {
  const found: Record<string, number> = {}
  for (const path of files(SRC)) {
    const name = relative(SRC, path).split('\\').join('/')
    if (PLUMBING.has(name)) continue
    const count = readFileSync(path, 'utf8').match(CALL)?.length ?? 0
    if (count > 0) found[name] = count
  }
  return Object.fromEntries(Object.entries(found).sort(([a], [b]) => a.localeCompare(b)))
}

describe('hand-written API calls', () => {
  it('only go down', () => {
    const now = counts()
    if (process.env.UPDATE_LEGACY_ALLOWLIST) writeFileSync(LIST, `${JSON.stringify(now, null, 2)}\n`)
    const allowed = JSON.parse(readFileSync(LIST, 'utf8')) as Record<string, number>
    const over = Object.entries(now)
      .filter(([file, count]) => count > (allowed[file] ?? 0))
      .map(([file, count]) => `${file}: ${count} (allowed ${allowed[file] ?? 0}); call the operation through api/ops.ts`)
    expect(over).toEqual([])
    const under = Object.entries(allowed)
      .filter(([file, count]) => (now[file] ?? 0) < count)
      .map(([file]) => file)
    expect(under, 'fewer calls than allowed: rerun with UPDATE_LEGACY_ALLOWLIST=1').toEqual([])
  })
})
