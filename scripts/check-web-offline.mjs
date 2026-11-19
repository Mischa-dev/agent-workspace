#!/usr/bin/env node
// H1.19: the built web app loads nothing from another origin, so a server
// on a closed network serves a whole app. Fails when `apps/web/dist` points
// a script, style, font or image at an absolute http(s) address: in HTML
// tags, in CSS `url(...)` and `@import`, and in scripts' own imports.
// Links a person follows (an `<a href>`, a URL in text) are fine.
//
//   node scripts/check-web-offline.mjs [dist]

import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

const dist = process.argv[2] ?? 'apps/web/dist'

function* files(directory) {
  for (const name of readdirSync(directory)) {
    const path = join(directory, name)
    if (statSync(path).isDirectory()) yield* files(path)
    else yield path
  }
}

const absolute = String.raw`(?:https?:)?\/\/[^\s"'()<>]+`
const rules = {
  html: [
    // <script src>, <img src>, <source src>, <link href> (styles, fonts, icons, preloads)
    new RegExp(String.raw`<(?:script|img|source|iframe|video|audio)\b[^>]*\bsrc\s*=\s*["']?(${absolute})`, 'gi'),
    new RegExp(String.raw`<link\b[^>]*\bhref\s*=\s*["']?(${absolute})`, 'gi'),
    new RegExp(String.raw`url\(\s*["']?(${absolute})`, 'gi'),
  ],
  css: [
    new RegExp(String.raw`url\(\s*["']?(${absolute})`, 'gi'),
    new RegExp(String.raw`@import\s+(?:url\()?\s*["']?(${absolute})`, 'gi'),
  ],
  js: [
    new RegExp(String.raw`\bimport\s*\(\s*["'\`](${absolute})`, 'g'),
    new RegExp(String.raw`\bimport\s+[^;]*?from\s*["'](${absolute})`, 'g'),
    new RegExp(String.raw`\bimportScripts\s*\(\s*["'](${absolute})`, 'g'),
  ],
}

const found = []
let checked = 0
for (const path of files(dist)) {
  const kind = path.endsWith('.html') ? 'html' : path.endsWith('.css') ? 'css' : /\.m?js$/.test(path) ? 'js' : null
  if (!kind) continue
  checked += 1
  const text = readFileSync(path, 'utf8')
  for (const rule of rules[kind]) {
    for (const match of text.matchAll(rule)) found.push(`${relative(dist, path)}: ${match[1]}`)
  }
}

if (checked === 0) {
  console.error(`web offline check: nothing built in ${dist}`)
  process.exit(1)
}
if (found.length > 0) {
  console.error('web offline check: the built app loads from another origin:')
  for (const line of found) console.error(`  ${line}`)
  process.exit(1)
}
console.log(`web offline check: ${checked} files load nothing from another origin`)
