import { spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'

import { budgetTable, measureWebAssets } from './web-budget-lib.mjs'

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

// WEB_BUDGET_SAVE keeps this report (tools/ci/checks.sh files it by commit);
// WEB_BUDGET_BASE names the base commit's, for the change column.
const save = process.env.WEB_BUDGET_SAVE
if (save) {
  mkdirSync(dirname(save), { recursive: true })
  writeFileSync(save, JSON.stringify(report))
}
const basePath = process.env.WEB_BUDGET_BASE
const base = basePath && existsSync(basePath) ? JSON.parse(readFileSync(basePath, 'utf8')) : null
process.stderr.write(`\nWeb asset budgets${base ? ` (base ${base.commit.slice(0, 10)})` : ''}:\n${budgetTable(report, base)}\n`)
if (report.violations.length) process.exitCode = 1
