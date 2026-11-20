import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { DECLARED_THINGS } from '../../pieces/declared'
import { Icon } from '../../ui'
import { thingQuery } from './thingQuery'

/** A part of a thing (a sheet row, a sticky) opens as the thing it's in. */
const PART_OF: Record<string, string> = { sheet_row: 'sheet', sticky: 'whiteboard' }

function wordFor(kind: string) {
  const word = DECLARED_THINGS.find((entry) => entry.decl.kind === kind)?.decl.one.toLowerCase() ?? kind.replace(/_/g, ' ')
  return `${/^[aeiou]/.test(word) ? 'an' : 'a'} ${word}`
}

/**
 * The thing a system entry says was made (C2.20): "a page: Launch notes",
 * read live with the reader's access. A thing the reader can't open is named
 * by its kind alone.
 */
export default function MadeThing({ id, kind }: { id: string; kind: string }) {
  const query = useQuery(thingQuery(PART_OF[kind] ?? kind, id))
  const thing = query.data?.thing
  if (!thing) return <>{wordFor(kind)}</>
  return (
    <>
      {`${wordFor(kind)}: `}
      <Link className="aw-event__thing" to={thing.href}>
        {thing.title}
      </Link>
    </>
  )
}

/**
 * What a message was made into (C2.20), one line each under the message. A
 * thing the reader can't open shows nothing at all.
 */
export function MadeIntoChips({ things }: { things: { kind: string; id: string }[] }) {
  return (
    <>
      {things.map((made) => (
        <MadeIntoChip id={made.id} key={`${made.kind}:${made.id}`} kind={made.kind} />
      ))}
    </>
  )
}

function MadeIntoChip({ id, kind }: { id: string; kind: string }) {
  const query = useQuery(thingQuery(PART_OF[kind] ?? kind, id))
  const thing = query.data?.thing
  if (!thing) return null
  return (
    <p className="aw-msg__made">
      <Icon name="link" size={12} /> Made into {`${wordFor(kind)}: `}
      <Link className="aw-event__thing" to={thing.href}>
        {thing.title}
      </Link>
    </p>
  )
}
