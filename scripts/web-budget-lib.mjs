import { gzipSync } from 'node:zlib'
import { lstatSync, readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'

export const WEB_ASSET_LIMITS = Object.freeze({
  // Raised from 500/160 KiB for the redesign's shell, components and icons (+62 KiB raw, +22 KiB gzip).
  // Account settings and the shared member picker bring the measured bundle to
  // 571,778 raw bytes and 180,817 gzip bytes. Keep the raw ceiling and allow
  // 2 KiB more gzip for these added routes and controls.
  // Google and WorkOS agent controls measure 574,498 raw / 181,586 gzip bytes.
  // Allow 2 KiB raw for these controls; retain the compressed transfer limit.
  // The agent interface (connected apps, thread status and board, DM threads,
  // pages, decisions, pins and search) measures 605,885 raw / 192,439 gzip
  // bytes, most of it in lazily loaded routes.
  // Mobile support (the service worker, one phone drawer, swipes, the keyboard
  // viewport, reconnect on resume) adds 9,350 raw / 5,358 gzip bytes over
  // 607,316 / 193,011, measuring 616,666 / 198,369; sw.js alone is 3,524 / 1,406.
  // Editing and removing messages, their history, reactions, live updates of
  // older pages and mark-unread (MCP Phase 3) measure 642,380 / 206,161, all
  // in the lazily loaded conversation routes.
  // The app shell (command menu, shortcuts, toasts with Undo, skeletons, tab
  // titles and badges, the thread side panel, live task patches) and the
  // Markdown renderer and editor measure 694,135 / 223,840; code highlighting
  // loads only with a code block.
  javascriptRawBytes: 680 * 1024,
  javascriptGzipBytes: 220 * 1024,
  // Raised from 72 KiB for the design system's five theme presets, components and page layouts.
  // Lowered from 104 KiB once pass two removed the legacy stylesheet.
  // The kit (toasts, command menu, shortcut sheet, skeletons) and Markdown
  // styles bring it to 112,083.
  cssRawBytes: 110 * 1024,
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
  const javascript = files.filter(({ file }) => file.endsWith('.js'))
  const css = files.filter(({ file }) => file.endsWith('.css'))
  const totals = {
    javascriptRawBytes: sum(javascript, 'bytes'),
    javascriptGzipBytes: sum(javascript, 'gzipBytes'),
    cssRawBytes: sum(css, 'bytes'),
  }
  const violations = Object.entries(limits)
    .filter(([name, limit]) => totals[name] > limit)
    .map(([name, limit]) => ({ name, observed: totals[name], limit }))
  return { compression: 'gzip level 9, per emitted asset', files, totals, limits, violations }
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
