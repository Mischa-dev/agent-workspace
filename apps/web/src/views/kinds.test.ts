/// <reference types="node" />
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

import { describe, expect, it } from 'vitest'

// One field and view vocabulary (T1.2): field types are written once, in
// fields/fieldModel.ts, and view kinds once, in views/kinds.ts; everything
// else (the module contract, collections) re-exports or narrows them.

const SRC = join(process.cwd(), 'src')

function files(folder: string): string[] {
  return readdirSync(folder).flatMap((name) => {
    const path = join(folder, name)
    if (statSync(path).isDirectory()) return files(path)
    return /\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name) ? [path] : []
  })
}

const defining = (pattern: RegExp) =>
  files(SRC)
    .filter((path) => pattern.test(readFileSync(path, 'utf8')))
    .map((path) => relative(SRC, path).split('\\').join('/'))

describe('one field and view vocabulary', () => {
  it('writes the field types once', () => {
    expect(defining(/\btype FieldType\s*=\s*\n?\s*\|?\s*'/)).toEqual(['fields/fieldModel.ts'])
  })

  it('writes the view kinds once; other view types narrow them', () => {
    expect(defining(/\btype View(Kind|Type)\s*=\s*\n?\s*\|?\s*'/)).toEqual(['views/kinds.ts'])
  })
})
