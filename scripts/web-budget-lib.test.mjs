import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { measureWebAssets } from './web-budget-lib.mjs'

function withDist(build) {
  const root = mkdtempSync(join(tmpdir(), 'agent-workspace-web-budget-'))
  try {
    mkdirSync(join(root, 'assets'))
    writeFileSync(
      join(root, 'index.html'),
      [
        '<script src="/boot.js"></script>',
        '<script type="module" crossorigin src="/assets/app.js"></script>',
        '<link rel="modulepreload" crossorigin href="/assets/ui.js">',
        '<link rel="stylesheet" crossorigin href="/assets/app.css">',
        '<link rel="icon" href="/icon.svg">',
      ].join('\n'),
    )
    writeFileSync(join(root, 'boot.js'), 'a;')
    writeFileSync(join(root, 'assets', 'app.js'), 'const value = 1;')
    writeFileSync(join(root, 'assets', 'ui.js'), 'export {};')
    writeFileSync(join(root, 'assets', 'app.css'), 'body { color: white; }')
    writeFileSync(join(root, 'icon.svg'), '<svg/>')
    return build(root)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
}

const roomy = {
  initialJavascriptRawBytes: 1024,
  initialJavascriptGzipBytes: 1024,
  initialCssRawBytes: 1024,
  chunkJavascriptRawBytes: 1024,
  chunkJavascriptGzipBytes: 1024,
  chunkCssRawBytes: 1024,
}

test('counts what index.html loads as the initial load', () => {
  withDist((root) => {
    const report = measureWebAssets(root, { ...roomy, initialJavascriptRawBytes: 20 })
    assert.deepEqual(report.initial, ['assets/app.css', 'assets/app.js', 'assets/ui.js', 'boot.js'])
    assert.equal(report.totals.initialJavascriptRawBytes, 28)
    assert.equal(report.totals.initialCssRawBytes, 22)
    assert.deepEqual(report.violations, [
      { name: 'initialJavascriptRawBytes', observed: 28, limit: 20 },
    ])
  })
})

test('measures gzip with content hashes replaced, so a size moves only when code does', () => {
  const plain = Array.from({ length: 40 }, (_, index) => `aaaa${String(index).padStart(4, '0')}`)
  const varied = Array.from({ length: 40 }, (_, index) =>
    createHash('sha256').update(`route ${index}`).digest('base64url').slice(0, 8),
  )
  const measure = (hashes) =>
    withDist((root) => {
      writeFileSync(
        join(root, 'assets', 'app.js'),
        hashes.map((hash, index) => `import('./Route${index}-${hash}.js');`).join('\n'),
      )
      for (const [index, hash] of hashes.entries()) {
        writeFileSync(join(root, 'assets', `Route${index}-${hash}.js`), `export const page = ${index};`)
      }
      return measureWebAssets(root, roomy).totals
    })
  const before = measure(plain)
  const after = measure(varied)
  assert.equal(after.initialJavascriptRawBytes, before.initialJavascriptRawBytes)
  assert.equal(after.initialJavascriptGzipBytes, before.initialJavascriptGzipBytes)
  assert.equal(after.javascriptGzipBytes, before.javascriptGzipBytes)
})

test('holds each lazy chunk to its own limit, not a total', () => {
  withDist((root) => {
    // Many small routes fit however many there are.
    for (let index = 0; index < 50; index += 1) {
      writeFileSync(join(root, 'assets', `Route${index}.js`), 'export const page = 1;')
    }
    assert.deepEqual(measureWebAssets(root, { ...roomy, chunkJavascriptRawBytes: 64 }).violations, [])

    writeFileSync(join(root, 'assets', 'Huge.js'), 'x'.repeat(200))
    writeFileSync(join(root, 'assets', 'Huge.css'), 'y'.repeat(200))
    const report = measureWebAssets(root, {
      ...roomy,
      chunkJavascriptRawBytes: 64,
      chunkCssRawBytes: 64,
    })
    assert.deepEqual(
      report.violations.map(({ name, file }) => [name, file]),
      [
        ['chunkJavascriptRawBytes', 'assets/Huge.js'],
        ['chunkCssRawBytes', 'assets/Huge.css'],
      ],
    )
  })
})

test('holds whiteboard chunks to their own limit', () => {
  withDist((root) => {
    mkdirSync(join(root, 'assets', 'whiteboard'))
    writeFileSync(join(root, 'assets', 'whiteboard', 'Canvas.js'), 'x'.repeat(200))
    writeFileSync(join(root, 'assets', 'whiteboard', 'Canvas.css'), 'y'.repeat(200))
    const limits = { ...roomy, chunkJavascriptRawBytes: 64, chunkCssRawBytes: 64 }
    assert.deepEqual(
      measureWebAssets(root, limits).violations.map(({ name }) => name),
      ['chunkJavascriptRawBytes', 'chunkCssRawBytes'],
    )
    const report = measureWebAssets(root, {
      ...limits,
      whiteboardChunkJavascriptRawBytes: 256,
      whiteboardChunkJavascriptGzipBytes: 256,
      whiteboardChunkCssRawBytes: 100,
    })
    assert.deepEqual(
      report.violations.map(({ name, file }) => [name, file]),
      [['whiteboardChunkCssRawBytes', 'assets/whiteboard/Canvas.css']],
    )
  })
})

test('holds the page editor to its own limit, and its stylesheet to the general one', () => {
  withDist((root) => {
    mkdirSync(join(root, 'assets', 'editor'))
    writeFileSync(join(root, 'assets', 'editor', 'LivePage.js'), 'x'.repeat(200))
    writeFileSync(join(root, 'assets', 'editor', 'LivePage.css'), 'y'.repeat(200))
    const limits = { ...roomy, chunkJavascriptRawBytes: 64, chunkCssRawBytes: 64 }
    assert.deepEqual(
      measureWebAssets(root, limits).violations.map(({ name }) => name),
      ['chunkJavascriptRawBytes', 'chunkCssRawBytes'],
    )
    const report = measureWebAssets(root, { ...limits, editorChunkJavascriptRawBytes: 256, editorChunkJavascriptGzipBytes: 256 })
    assert.deepEqual(
      report.violations.map(({ name, file }) => [name, file]),
      [['chunkCssRawBytes', 'assets/editor/LivePage.css']],
    )
  })
})
