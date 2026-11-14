import type { EmbedDecl, PieceType, ThingDecl } from '../modules/contract'

// What the manifests in `modules/` declare for the pieces: every kind of
// thing and every embed, with the module that declares it. Loaded with the
// pieces gallery only.

const THINGS = import.meta.glob<ThingDecl[]>(['../../../../modules/*.json', '!../../../../modules/presets.json'], {
  eager: true,
  import: 'things',
})
const EMBEDS = import.meta.glob<EmbedDecl[]>(['../../../../modules/*.json', '!../../../../modules/presets.json'], {
  eager: true,
  import: 'embeds',
})

const idOf = (path: string) => path.slice(path.lastIndexOf('/') + 1).replace(/\.json$/, '')

/** Every kind of thing, with the module that declares it. */
export const DECLARED_THINGS = Object.entries(THINGS).flatMap(([path, things]) =>
  (things ?? []).map((decl) => ({ module: idOf(path), decl })),
)

/** Every embed a manifest declares, with its module. */
export const DECLARED_EMBEDS = Object.entries(EMBEDS).flatMap(([path, embeds]) =>
  (embeds ?? []).map((embed) => ({ module: idOf(path), embed })),
)

/** Embeds by module, in manifest order. */
export function embedsByModule() {
  const found = new Map<string, typeof DECLARED_EMBEDS>()
  for (const entry of DECLARED_EMBEDS) found.set(entry.module, [...(found.get(entry.module) ?? []), entry])
  return [...found]
}

export const EVERY_PIECE: PieceType[] = ['list', 'table', 'board', 'detail', 'fields', 'form', 'card', 'actions']
