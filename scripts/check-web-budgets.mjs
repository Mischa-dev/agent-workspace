import { spawnSync } from 'node:child_process'
import { resolve } from 'node:path'

import { measureWebAssets } from './web-budget-lib.mjs'

const distRoot = resolve(process.cwd(), process.argv[2] ?? 'apps/web/dist')
const commitResult = spawnSync('git', ['rev-parse', 'HEAD'], {
  cwd: process.cwd(),
  encoding: 'utf8',
  stdio: ['ignore', 'pipe', 'ignore'],
})
const report = {
  schemaVersion: 1,
  commit: commitResult.status === 0 ? commitResult.stdout.trim() : 'unknown',
  distRoot: 'apps/web/dist',
  ...measureWebAssets(distRoot),
}

process.stdout.write(`${JSON.stringify(report, null, 2)}\n`)
if (report.violations.length) process.exitCode = 1
