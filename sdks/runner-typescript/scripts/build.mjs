// Strips the types from src/*.ts into dist/*.js with Node's own stripper, so
// the package needs no compiler and no dependencies. Types stay in src, which
// package.json points TypeScript at; `./x.ts` imports become `./x.js`.
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { stripTypeScriptTypes } from 'node:module'

const src = new URL('../src/', import.meta.url)
const dist = new URL('../dist/', import.meta.url)
mkdirSync(dist, { recursive: true })
for (const name of readdirSync(src).filter((file) => file.endsWith('.ts'))) {
  const source = readFileSync(new URL(name, src), 'utf8')
  const stripped = stripTypeScriptTypes(source, { mode: 'strip' }).replace(/(from\s+'\.\/[\w-]+)\.ts'/g, "$1.js'")
  writeFileSync(new URL(name.replace(/\.ts$/, '.js'), dist), stripped)
}
