import { isApiError } from '../../api/errors'
import type { Piece, Thing, ThingDecl } from '../../modules/contract'
import { DECLARED_EMBEDS, DECLARED_THINGS } from '../declared'

// What the embeds modules register for things (W1.26) share: the manifest's
// declaration of a kind and its card, and a piece that shows only the fields
// the things fill.

export function declOf(kind: string): ThingDecl | null {
  return DECLARED_THINGS.find((entry) => entry.decl.kind === kind)?.decl ?? null
}

/** The piece of this type the manifests declare for showing `thing` in a page. */
export function embedPiece(thing: string, type: 'card' | 'list' | 'board'): Piece | null {
  return DECLARED_EMBEDS.find((entry) => entry.embed.thing === thing && entry.embed.piece.type === type)?.embed.piece ?? null
}

/** The piece's fields that the things have values for, so a card shows no empty rows. */
export function filled(piece: Piece, things: Thing[]): Piece {
  const has = (id: string) =>
    things.some((thing) => (id === 'status' ? thing.status : id === 'owner' ? thing.owner : thing.fields?.[id]) != null)
  return { ...piece, fields: (piece.fields ?? []).filter(has) }
}

/** Retry server errors twice; a refusal or a missing thing stays as it is. */
export const retryServerErrors = (count: number, error: unknown) => !(isApiError(error) && error.status < 500) && count < 2
