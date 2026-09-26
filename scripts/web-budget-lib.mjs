import { gzipSync } from 'node:zlib'
import { lstatSync, readdirSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'

export const WEB_ASSET_LIMITS = Object.freeze({
  // Raised from 500/160 KiB for the redesign's shell, components and icons (+62 KiB raw, +22 KiB gzip).
  // Pass two (work items, settings, Home) used most of the rest: about 6 KiB raw and 1.5 KiB gzip remain.
  javascriptRawBytes: 560 * 1024,
  javascriptGzipBytes: 176 * 1024,
  // Raised from 72 KiB for the design system's five theme presets, components and page layouts.
  // Lowered from 104 KiB once pass two removed the legacy stylesheet.
  cssRawBytes: 100 * 1024,
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
