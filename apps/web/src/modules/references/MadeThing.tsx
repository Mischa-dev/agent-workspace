import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { DECLARED_THINGS } from '../../pieces/declared'
import { thingQuery } from './thingQuery'

/**
 * The thing a system entry says was made (C2.20): "a page: Launch notes",
 * read live with the reader's access. A thing the reader can't open is named
 * by its kind alone.
 */
export default function MadeThing({ id, kind }: { id: string; kind: string }) {
  const query = useQuery(thingQuery(kind, id))
  const word = DECLARED_THINGS.find((entry) => entry.decl.kind === kind)?.decl.one.toLowerCase() ?? kind.replace(/_/g, ' ')
  const thing = query.data?.thing
  const article = /^[aeiou]/.test(word) ? 'an' : 'a'
  if (!thing) return <>{`${article} ${word}`}</>
  return (
    <>
      {`${article} ${word}: `}
      <Link className="aw-event__thing" to={thing.href}>
        {thing.title}
      </Link>
    </>
  )
}
