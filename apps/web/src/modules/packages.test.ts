/// <reference types="node" />
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { PACKAGE_CONTENT_KINDS, type Package, partsNeedingAdmin } from './packages'

// The server writes this schema (apps/server/src/packages/format.rs, with
// OPS_BLESS=1); Vitest runs from apps/web.
const SCHEMA = join(process.cwd(), '../../modules/packages/package.schema.json')

type Schema = {
  properties: Record<string, unknown>
  required: string[]
  $defs?: Record<string, { enum?: string[]; oneOf?: { const?: string; enum?: string[] }[]; properties?: Record<string, unknown> }>
  definitions?: Record<string, { enum?: string[]; oneOf?: { const?: string; enum?: string[] }[]; properties?: Record<string, unknown> }>
}

const schema = JSON.parse(readFileSync(SCHEMA, 'utf8')) as Schema
const defs = schema.$defs ?? schema.definitions ?? {}

function values(name: string): string[] {
  const def = defs[name]
  if (!def) return []
  if (def.enum) return def.enum
  return (def.oneOf ?? []).flatMap((option) => option.enum ?? (option.const ? [option.const] : []))
}

describe('the package format', () => {
  it('names the same fields as the server', () => {
    const fields: (keyof Package)[] = [
      'format',
      'id',
      'version',
      'name',
      'description',
      'publisher',
      'target',
      'requires',
      'contents',
      'needsAdmin',
      'digest',
    ]
    expect(Object.keys(schema.properties).sort()).toEqual([...fields].sort())
    expect([...schema.required].sort()).toEqual(['contents', 'format', 'id', 'name', 'target', 'version'])
  })

  it('has the same part kinds and targets', () => {
    expect(values('ContentKind').sort()).toEqual([...PACKAGE_CONTENT_KINDS].sort())
    expect(values('PackageTarget').sort()).toEqual(['space', 'workspace'])
    expect(Object.keys(defs.Content?.properties ?? {}).sort()).toEqual(['body', 'key', 'kind', 'path'])
  })

  it('lists the parts that need an admin', () => {
    const pkg: Package = {
      format: 'agent-workspace-package/1',
      id: 'studio.playtests',
      version: '1.0.0',
      name: 'Playtests',
      target: 'space',
      contents: [
        { kind: 'collection', key: 'feedback', body: {} },
        { kind: 'form', key: 'feedback', body: { collection: '@feedback' } },
      ],
      needsAdmin: ['form:feedback'],
    }
    expect(partsNeedingAdmin(pkg).map((part) => part.kind)).toEqual(['form'])
  })
})
