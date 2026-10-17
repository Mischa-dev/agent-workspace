// Strips the types from src/index.ts into dist/index.js with Node's own
// stripper, so the package needs no compiler and no dependencies. Types stay
// in src/index.ts, which package.json points TypeScript at.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { stripTypeScriptTypes } from 'node:module'

const source = readFileSync(new URL('../src/index.ts', import.meta.url), 'utf8')
mkdirSync(new URL('../dist/', import.meta.url), { recursive: true })
writeFileSync(new URL('../dist/index.js', import.meta.url), stripTypeScriptTypes(source, { mode: 'strip' }))
