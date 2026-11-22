import { Link } from 'react-router-dom'

import { isApiError } from '../../api/errors'
import type { Piece, Thing, ThingDecl } from '../../modules/contract'
import { Button, Icon } from '../../ui'
import { DECLARED_EMBEDS, DECLARED_THINGS } from '../declared'
import './thingEmbed.css'

// What the embeds modules register for things (W1.26) share: the manifest's
// declaration of a kind and its card, a problem line, and a piece that shows
// only the fields the things fill.

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

export function EmbedLink({ href, label }: { href: string; label: string }) {
  return (
    <Link className="aw-embed__link" to={href}>
      <Icon name="link" size={16} />
      <span>{label || href}</span>
    </Link>
  )
}

export function EmbedProblem({ error, href, label, onRetry }: { error: unknown; href: string; label: string; onRetry: () => void }) {
  const hidden = isApiError(error) && (error.status === 404 || error.status === 403)
  return (
    <p className="aw-membed__problem">
      <Icon name="link" size={16} />
      <span>
        {hidden ? (
          <>
            <Link to={href}>{label || 'This'}</Link> isn’t here, or you can’t see it.
          </>
        ) : (
          `${label || 'This'} didn’t load.`
        )}
      </span>
      {hidden ? null : (
        <Button onClick={onRetry} size="sm">
          Try again
        </Button>
      )}
    </p>
  )
}

/** Retry server errors twice; a refusal or a missing thing stays as it is. */
export const retryServerErrors = (count: number, error: unknown) => !(isApiError(error) && error.status < 500) && count < 2
