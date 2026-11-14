// The package format (`agent-workspace-package/1`, M1 section 3.6) as the
// web app reads it. The server writes the JSON Schema of the inline form to
// modules/packages/package.schema.json; packages.test.ts checks these types
// against it, so the two sides can't drift.

export const PACKAGE_FORMAT = 'agent-workspace-package/1'

/** A part of a package, by kind. */
export const PACKAGE_CONTENT_KINDS = [
  'switches',
  'layout',
  'role',
  'space',
  'channels',
  'module',
  'collection',
  'form',
  'automation',
  'page',
  'skill',
  'agent',
] as const

export type PackageContentKind = (typeof PACKAGE_CONTENT_KINDS)[number]

export type PackageTarget = 'space' | 'workspace'

export type PackageContent = {
  kind: PackageContentKind
  key: string
  path?: string
  body?: Record<string, unknown>
}

export type Package = {
  format: typeof PACKAGE_FORMAT
  id: string
  version: string
  name: string
  description?: string
  publisher?: { name: string; url?: string }
  target: PackageTarget
  requires?: { contract?: string; modules?: string[] }
  contents: PackageContent[]
  needsAdmin?: string[]
  digest?: string
}

/** The parts that need an admin to install, as "kind:key". */
export function partsNeedingAdmin(pkg: Package): PackageContent[] {
  const needed = new Set(pkg.needsAdmin ?? [])
  return pkg.contents.filter((part) => needed.has(`${part.kind}:${part.key}`))
}
