import { gzipSync } from 'node:zlib'
import { existsSync, lstatSync, readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'

// The budget holds the code every page load downloads (what index.html loads
// before the app can paint) and each lazily loaded chunk on its own, never a
// grand total, so a branch that adds a lazy route doesn't edit this file.
//
// Policy (W1.15): each limit is the size measured when it was last set plus
// 10%, and CI prints every limit's measured size (and, when the base
// commit's report is at hand, the change against it) on every run, so growth
// shows long before a limit is crossed. A run fails only when one is. Raise a
// limit by measuring again and adding 10%, and say why in the commit message:
// the history of each number is in the git log, not here.
//
// Measured November 2026 (W1.15: the data router in the shell; feature
// stylesheets moved from the first paint into the routes that use them).
export const WEB_ASSET_LIMITS = Object.freeze({
  // index.html's scripts and module preloads: 486,736 raw / 154,185 gzip.
  initialJavascriptRawBytes: 535_410,
  initialJavascriptGzipBytes: 169_604,
  // index.html's stylesheets (tokens, base, components, shell, kit): 61,731.
  initialCssRawBytes: 67_905,
  // Any other script (each lazy route or shared lazy chunk, sw.js). The
  // largest measured 56,135 raw (collections) and 16,415 gzip (task panel).
  chunkJavascriptRawBytes: 61_749,
  chunkJavascriptGzipBytes: 18_057,
  // Any stylesheet a lazy chunk loads: the largest, the task pages', 32,641.
  chunkCssRawBytes: 35_906,
  // Whiteboards draw with Excalidraw, which apps/web/vite.config.ts puts under
  // assets/whiteboard/ with everything only it pulls in. Nothing else loads
  // them. The largest, the font subsetter for exports, is 1,821,039 raw /
  // 735,014 gzip (740,312 with the CI host's zlib, which the gzip limit
  // starts from); its stylesheet is 142,020.
  whiteboardChunkJavascriptRawBytes: 2_003_143,
  whiteboardChunkJavascriptGzipBytes: 814_344,
  whiteboardChunkCssRawBytes: 156_222,
  // The live page editor (Tiptap, Yjs and the page's own schema and sync),
  // under assets/editor/: one chunk of 532,920 raw / 161,086 gzip. Its
  // stylesheet is held to chunkCssRawBytes.
  editorChunkJavascriptRawBytes: 586_212,
  editorChunkJavascriptGzipBytes: 177_195,
})

// Chunks held to their own limits, by where vite.config.ts puts them.
const OWN_LIMITS = [
  ['assets/whiteboard/', 'whiteboard'],
  ['assets/editor/', 'editor'],
]

/** The limit for one kind of file: its folder's own (`whiteboardChunkCssRawBytes`) if set, otherwise the general one. */
function limitName(file, generalName, limits) {
  const owner = OWN_LIMITS.find(([prefix]) => file.startsWith(prefix))?.[1]
  const own = owner ? `${owner}${generalName[0].toUpperCase()}${generalName.slice(1)}` : null
  return own && limits[own] !== undefined ? own : generalName
}

export function measureWebAssets(distRoot, limits = WEB_ASSET_LIMITS) {
  const paths = walkRegularFiles(distRoot)
  const names = paths.map((absolutePath) => relative(distRoot, absolutePath).split('\\').join('/'))
  const hashes = contentHashes(names)
  const files = paths.map((absolutePath, index) => {
    const file = names[index]
    const bytes = readFileSync(absolutePath)
    const measured = /\.(?:js|css)$/.test(file) ? withoutHashes(bytes, hashes) : bytes
    return {
      file,
      bytes: bytes.length,
      gzipBytes: gzipSync(measured, { level: 9, mtime: 0 }).length,
    }
  })
  const initial = initialAssets(distRoot)
  const isInitial = ({ file }) => initial.has(file)
  const javascript = files.filter(({ file }) => file.endsWith('.js'))
  const css = files.filter(({ file }) => file.endsWith('.css'))
  const totals = {
    initialJavascriptRawBytes: sum(javascript.filter(isInitial), 'bytes'),
    initialJavascriptGzipBytes: sum(javascript.filter(isInitial), 'gzipBytes'),
    initialCssRawBytes: sum(css.filter(isInitial), 'bytes'),
    javascriptRawBytes: sum(javascript, 'bytes'),
    javascriptGzipBytes: sum(javascript, 'gzipBytes'),
    cssRawBytes: sum(css, 'bytes'),
  }
  const violations = []
  for (const name of ['initialJavascriptRawBytes', 'initialJavascriptGzipBytes', 'initialCssRawBytes']) {
    if (limits[name] !== undefined && totals[name] > limits[name]) {
      violations.push({ name, observed: totals[name], limit: limits[name] })
    }
  }
  const chunkChecks = [
    [javascript, 'bytes', 'chunkJavascriptRawBytes'],
    [javascript, 'gzipBytes', 'chunkJavascriptGzipBytes'],
    [css, 'bytes', 'chunkCssRawBytes'],
  ]
  for (const [records, key, chunkName] of chunkChecks) {
    for (const record of records) {
      const name = limitName(record.file, chunkName, limits)
      const limit = limits[name]
      if (limit === undefined) continue
      if (!isInitial(record) && record[key] > limit) {
        violations.push({ name, file: record.file, observed: record[key], limit })
      }
    }
  }
  return {
    compression: 'gzip level 9, per emitted asset, with content hashes in file names replaced by one placeholder',
    initial: [...initial].sort(),
    files,
    totals,
    limits,
    violations,
  }
}

// Emitted names end in an eight-character content hash (index-b5whv8zU.js),
// and a chunk that loads others names them. The letters are random, so their
// gzip size changes whenever any chunk does; measuring with every hash
// replaced by one placeholder makes a size move only when code does. The raw
// size is unchanged, since the placeholder is as long as a hash.
const HASHED_NAME = /-([A-Za-z0-9_-]{8})\.[A-Za-z0-9]+$/
const HASH_PLACEHOLDER = '00000000'

function contentHashes(names) {
  const hashes = new Set()
  for (const name of names) {
    const hash = HASHED_NAME.exec(name)?.[1]
    if (hash) hashes.add(hash)
  }
  return hashes
}

function withoutHashes(bytes, hashes) {
  if (hashes.size === 0) return bytes
  const text = bytes.toString('latin1')
  const measured = text.replace(/-([A-Za-z0-9_-]{8})\./g, (match, hash) =>
    hashes.has(hash) ? `-${HASH_PLACEHOLDER}.` : match,
  )
  return measured === text ? bytes : Buffer.from(measured, 'latin1')
}

// The scripts, module preloads and stylesheets index.html loads itself.
function initialAssets(distRoot) {
  const indexPath = join(distRoot, 'index.html')
  const initial = new Set()
  if (!existsSync(indexPath)) return initial
  const html = readFileSync(indexPath, 'utf8')
  for (const [tag] of html.matchAll(/<(?:script|link)\b[^>]*>/gi)) {
    const isScript = /^<script/i.test(tag)
    const rel = /\brel\s*=\s*["']?([^"'\s>]+)/i.exec(tag)?.[1]?.toLowerCase()
    if (!isScript && rel !== 'modulepreload' && rel !== 'stylesheet') continue
    const reference = /\b(?:src|href)\s*=\s*["']?([^"'\s>]+)/i.exec(tag)?.[1]
    if (!reference || /^[a-z][a-z0-9+.-]*:/i.test(reference) || reference.startsWith('//')) continue
    initial.add(reference.replace(/^\.?\//, '').split(/[?#]/)[0])
  }
  return initial
}

function walkRegularFiles(root) {
  const pending = [root]
  const files = []
  while (pending.length) {
    const directory = pending.pop()
    for (const name of readdirSync(directory).sort().reverse()) {
      const path = join(directory, name)
      const metadata = lstatSync(path)
      if (metadata.isSymbolicLink()) {
        throw new Error(`web artifact contains a symbolic link: ${relative(root, path)}`)
      }
      if (metadata.isDirectory()) pending.push(path)
      else if (metadata.isFile()) files.push(path)
      else throw new Error(`web artifact contains an unsupported entry: ${relative(root, path)}`)
    }
  }
  return files.sort()
}

function sum(records, key) {
  return records.reduce((total, record) => total + record[key], 0)
}

/** Every limit's measured value in one report: the largest file for a per-chunk limit. */
export function limitValues(report) {
  const values = {}
  for (const name of ['initialJavascriptRawBytes', 'initialJavascriptGzipBytes', 'initialCssRawBytes']) {
    values[name] = report.totals[name]
  }
  const initial = new Set(report.initial)
  const chunkChecks = [
    ['.js', 'bytes', 'chunkJavascriptRawBytes'],
    ['.js', 'gzipBytes', 'chunkJavascriptGzipBytes'],
    ['.css', 'bytes', 'chunkCssRawBytes'],
  ]
  for (const [extension, key, chunkName] of chunkChecks) {
    for (const record of report.files) {
      if (initial.has(record.file) || !record.file.endsWith(extension)) continue
      const name = limitName(record.file, chunkName, report.limits)
      values[name] = Math.max(values[name] ?? 0, record[key])
    }
  }
  return values
}

/**
 * A Markdown table of every limit: the base commit's value (when its report
 * is given), this one's, the change, the limit and the room left.
 */
export function budgetTable(head, base = null) {
  const headValues = limitValues(head)
  const baseValues = base ? limitValues(base) : {}
  const names = Object.keys(head.limits).filter((name) => headValues[name] !== undefined)
  const format = (value) => (value === undefined ? '' : value.toLocaleString('en-US'))
  const signed = (value) => (value > 0 ? `+${format(value)}` : value === 0 ? '0' : `-${format(-value)}`)
  const rows = [
    base ? '| Limit | Base | Head | Change | Limit | Room |' : '| Limit | Head | Limit | Room |',
    base ? '| --- | ---: | ---: | ---: | ---: | ---: |' : '| --- | ---: | ---: | ---: |',
  ]
  for (const name of names) {
    const value = headValues[name]
    const limit = head.limits[name]
    const room = `${(((limit - value) / limit) * 100).toFixed(1)}%`
    if (base) {
      const before = baseValues[name]
      const change = before === undefined ? '' : signed(value - before)
      rows.push(`| ${name} | ${format(before)} | ${format(value)} | ${change} | ${format(limit)} | ${room} |`)
    } else {
      rows.push(`| ${name} | ${format(value)} | ${format(limit)} | ${room} |`)
    }
  }
  return rows.join('\n')
}

