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
  // 136,131 to 419,086 / 137,030 (October 2026). Layouts bring it to
  // 422,207 / 136,850. The rail's own order and dragging, sidebar and panel
  // widths set before the first paint, offline sends from any page and more
  // remembered places bring it to 428,622 / 138,911; the resize edges and
  // the dialogs for comments, labels and decisions load when first used.
  // Modules (AWD-75) put module gating in the shell: the page gate, the
  // shortcut check, settings navigation and the rail, each reading only its
  // own kind of manifest claims (vite.config.ts moduleProjections), with the
  // turned-off page and the palette's claims loading lazily. That measured
  // 433,897 / 140,611 (November 2026), so the gzip limit went from 136 to
  // 138 KB by decision; the raw limit is unchanged.
  // Moving tasks between projects (AWD-102) must forget an unsent or
  // unanswered move at once on every sign-in and sign-out, so that small
  // store (moveStore.ts) is in the shell; sending and checking moves load
  // with the Move dialog. Measured from the built assets (not a browser's
  // network transfer): 433,999 / 140,955 before it; 435,731 / 141,719 with
  // the move code in the shell; 434,232 / 141,081 once split, 1,499 raw /
  // 638 gzip bytes less, still 56 raw bytes over 424 KB. The raw limit went
  // up 256 bytes by decision (November 2026); gzip is unchanged.
  // The entry names about 270 lazy chunks, each with eight random letters
  // of content hash, so the same shell gzipped up to a few dozen bytes
  // larger or smaller whenever any lazy chunk changed: a fix in the page
  // editor alone measured 141,323 against main's 141,306. Gzip sizes are now
  // taken with every hash replaced by the same placeholder (withoutHashes),
  // which measured 139,036 for both, 2,270 less; the gzip limit came down
  // by that much, so it is exactly as full as before (November 2026).
  initialJavascriptRawBytes: 424 * 1024 + 256,
  initialJavascriptGzipBytes: 138 * 1024 - 2270,
  // index.html's stylesheets: the design system's five theme presets,
  // components and page layouts. Measured 101,925 raw bytes; the UI kit
  // (toasts, command menu, shortcut sheet, skeletons) and Markdown styles
  // bring it to 112,083. The design system pass moved every hand-typed size,
  // weight and gap onto tokens (var(--space-1-5) is longer than 6px) and added
  // the scales themselves: 114,898 (October 2026).
  initialCssRawBytes: 116 * 1024,
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
  // The live page editor (AWD-53): Tiptap on ProseMirror, Yjs and the page's
  // own schema and sync, which apps/web/vite.config.ts puts under
  // assets/editor/. Only a page opened to write in loads it, as one chunk of
  // 524,864 raw / 158,581 gzip bytes (Tiptap 3.33, Yjs 13.6, November 2026).
  // Its stylesheet is small and held to chunkCssRawBytes.
  editorChunkJavascriptRawBytes: 544 * 1024,
  editorChunkJavascriptGzipBytes: 168 * 1024,
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
