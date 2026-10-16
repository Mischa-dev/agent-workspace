import { gzipSync } from 'node:zlib'
import { existsSync, lstatSync, readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'

// The budget holds the code every page load downloads (what index.html loads
// before the app can paint) and each lazily loaded chunk on its own, never a
// grand total, so a branch that adds a lazy route doesn't edit this file.
// Raise a limit only when that part really grows, and say why next to it.
export const WEB_ASSET_LIMITS = Object.freeze({
  // index.html's scripts and module preloads: the entry chunk, the shared UI
  // chunk and theme-boot.js. Measured 388,329 raw / 120,899 gzip bytes when
  // the budget moved to per-chunk limits (October 2026). The app shell (command
  // menu, shortcuts, toasts with Undo, tab titles and badges, live task
  // patches) brings it to 423,687 / 130,474; Markdown loads with its routes.
  initialJavascriptRawBytes: 420 * 1024,
  initialJavascriptGzipBytes: 132 * 1024,
  // index.html's stylesheets: the design system's five theme presets,
  // components and page layouts. Measured 101,925 raw bytes; the UI kit
  // (toasts, command menu, shortcut sheet, skeletons) and Markdown styles
  // bring it to 112,083.
  initialCssRawBytes: 112 * 1024,
  // Any other script (each lazy route or shared lazy chunk, sw.js). The
  // largest, project settings, measured 35,379 raw / 9,320 gzip bytes.
  // Tasks at scale (W3) made the task list page (list, board, filters, bulk
  // edits) and the task panel the largest lazy routes: 54,755 raw / 17,390
  // gzip and 54,619 / 15,346 bytes.
  chunkJavascriptRawBytes: 56 * 1024,
  chunkJavascriptGzipBytes: 18 * 1024,
  // Any stylesheet a lazy chunk loads.
  chunkCssRawBytes: 32 * 1024,
})

export function measureWebAssets(distRoot, limits = WEB_ASSET_LIMITS) {
  const files = walkRegularFiles(distRoot).map((absolutePath) => {
    const bytes = readFileSync(absolutePath)
    return {
      file: relative(distRoot, absolutePath).split('\\').join('/'),
      bytes: bytes.length,
      gzipBytes: gzipSync(bytes, { level: 9, mtime: 0 }).length,
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
  for (const [records, key, name] of chunkChecks) {
    const limit = limits[name]
    if (limit === undefined) continue
    for (const record of records) {
      if (!isInitial(record) && record[key] > limit) {
        violations.push({ name, file: record.file, observed: record[key], limit })
      }
    }
  }
  return {
    compression: 'gzip level 9, per emitted asset',
    initial: [...initial].sort(),
    files,
    totals,
    limits,
    violations,
  }
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
