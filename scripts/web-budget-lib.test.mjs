import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { measureWebAssets } from './web-budget-lib.mjs'

test('measures every emitted asset and applies aggregate limits', () => {
  const root = mkdtempSync(join(tmpdir(), 'agent-workspace-web-budget-'))
  try {
    mkdirSync(join(root, 'assets'))
    writeFileSync(join(root, 'index.html'), '<main>bounded</main>')
    writeFileSync(join(root, 'assets', 'app.js'), 'const value = 1;')
    writeFileSync(join(root, 'assets', 'app.css'), 'body { color: white; }')
    const report = measureWebAssets(root, {
      javascriptRawBytes: 8,
      javascriptGzipBytes: 1024,
      cssRawBytes: 1024,
    })
    assert.deepEqual(
      report.files.map(({ file }) => file),
      ['assets/app.css', 'assets/app.js', 'index.html'],
    )
    assert.equal(report.totals.javascriptRawBytes, 16)
    assert.deepEqual(report.violations, [
      { name: 'javascriptRawBytes', observed: 16, limit: 8 },
    ])
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
