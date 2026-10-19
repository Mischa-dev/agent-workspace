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
  // Tasks at scale and the inbox's rail tile and route bring it to about
  // 430,500 raw; the inbox page itself loads with its route. The settings
  // area (Settings in the rail and command menu, the account's saved theme)
  // brings it to 431,830 / 135,187; its pages and two-factor sign-in load lazily.
  // The polish pass added plain error sentences, shortcut labels for each
  // keyboard, confirm dialogs and steadier sidebars, and moved 9 KB of
  // conversation calls the shell doesn't need out of it: from 418,331 /
  // 136,131 to 419,086 / 137,030 (October 2026).
  initialJavascriptRawBytes: 424 * 1024,
  initialJavascriptGzipBytes: 135 * 1024,
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
  // Whiteboards draw with Excalidraw, which apps/web/vite.config.ts puts under
  // assets/whiteboard/ with everything only it pulls in. Nothing else loads
  // them. Opening a board fetches its canvas (550,932 raw / 162,306 gzip) and
  // one shared chunk (532,641 / 175,934); the rest load only when a board
  // uses them. The largest, the font subsetter for exports, is 1,821,039 raw /
  // 735,032 gzip; its stylesheet is 142,020. Excalidraw 0.18.1, October 2026.
  // The same file gzips to 740,312 on the NixOS CI host (another zlib), so
  // the gzip limit leaves room for that.
  whiteboardChunkJavascriptRawBytes: 1792 * 1024,
  whiteboardChunkJavascriptGzipBytes: 752 * 1024,
  whiteboardChunkCssRawBytes: 140 * 1024,
})

const WHITEBOARD_PREFIX = 'assets/whiteboard/'

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
    [javascript, 'bytes', 'chunkJavascriptRawBytes', 'whiteboardChunkJavascriptRawBytes'],
    [javascript, 'gzipBytes', 'chunkJavascriptGzipBytes', 'whiteboardChunkJavascriptGzipBytes'],
    [css, 'bytes', 'chunkCssRawBytes', 'whiteboardChunkCssRawBytes'],
  ]
  for (const [records, key, chunkName, whiteboardName] of chunkChecks) {
    for (const record of records) {
      const name = record.file.startsWith(WHITEBOARD_PREFIX) && limits[whiteboardName] !== undefined ? whiteboardName : chunkName
      const limit = limits[name]
      if (limit === undefined) continue
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
