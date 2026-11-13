import { useQuery } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import { Link } from 'react-router-dom'

import { Icon } from '../../ui'
import type { ThingAddress } from './thingLinks'
import { thingIcon, thingQuery } from './thingQuery'
import './references.css'

/**
 * A link to a task, page, whiteboard or conversation, with its title and
 * status as the reader can see them now. Until it loads, or where the reader
 * can't see it, it stays the plain link that was written.
 */
export default function ThingChip({ address, bare, children }: { address: ThingAddress; bare: boolean; children: ReactNode }) {
  const query = useQuery(thingQuery(address.kind, address.id))
  const thing = query.data?.thing
  if (!thing) return <Link to={address.path}>{children}</Link>
  return (
    <Link className="aw-thingchip" title={thing.title} to={address.path}>
      <Icon className="aw-thingchip__icon" name={thingIcon(thing.kind)} size={14} />
      <span className="aw-thingchip__title">{bare ? thing.title : children}</span>
      {thing.status ? <span className="aw-thingchip__status">· {thing.status}</span> : null}
    </Link>
  )
}
