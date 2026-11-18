import { useQuery } from '@tanstack/react-query'

import type { Piece, ThingDecl } from '../contract'
import { DECLARED_EMBEDS, DECLARED_THINGS } from '../../pieces/declared'
import { PieceView } from '../../pieces/Pieces'
import type { ThingAddress } from './thingLinks'
import { thingQuery } from './thingQuery'
import './references.css'

/** The kind's own card, as its module's manifest declares it; else its status and owner. */
function cardFor(kind: string): { decl: ThingDecl; piece: Piece } | null {
  const decl = DECLARED_THINGS.find((entry) => entry.decl.kind === kind)?.decl
  if (!decl) return null
  const embed = DECLARED_EMBEDS.find((entry) => entry.embed.piece.type === 'card' && entry.embed.piece.thing === kind)
  // Under a message the card only shows; opening it is its title's link.
  const piece = embed ? { ...embed.embed.piece, actions: [] } : { type: 'card' as const, thing: kind, fields: ['status', 'owner'] }
  return { decl, piece }
}

function LinkCard({ address }: { address: ThingAddress }) {
  const query = useQuery(thingQuery(address.kind, address.id))
  const thing = query.data?.thing
  const card = cardFor(address.kind)
  // Only what the reader can see now: a thing they can't read draws nothing.
  if (!thing || !card) return null
  return (
    <li className="aw-linkcards__item">
      <PieceView decl={card.decl} label={`${card.decl.one}: ${thing.title}`} piece={card.piece} things={[{ ...thing, href: address.path }]} />
    </li>
  )
}

/**
 * Internal link cards (C2.23): under a message, a card for each thing in this
 * app its text links to, drawn by the kind's card piece and read live with
 * the reader's access. Nothing is fetched from outside and no text is copied
 * into the message.
 */
export default function LinkCards({ addresses }: { addresses: ThingAddress[] }) {
  return (
    <ul aria-label="Linked here" className="aw-linkcards">
      {addresses.map((address) => (
        <LinkCard address={address} key={`${address.kind}:${address.id}`} />
      ))}
    </ul>
  )
}
