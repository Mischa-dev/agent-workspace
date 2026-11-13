import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { Icon, Row, RowList, Section } from '../../ui'
import { thingIcon, thingQuery } from './thingQuery'
import './references.css'

const KIND_WORDS: Record<string, string> = {
  task: 'Task',
  page: 'Page',
  decision: 'Decision',
  whiteboard: 'Whiteboard',
  thread: 'Thread',
  channel: 'Channel',
  conversation: 'Conversation',
  project: 'Project',
  record: 'Record',
}

/**
 * Where a task, page, whiteboard or record is mentioned (AWD-58, AWD-112): the conversations,
 * pages and tasks that link to it, as far as the reader can see them. It
 * shows nothing until there's something to list.
 */
export default function ReferencedIn({ kind, id }: { kind: string; id: string }) {
  // Its own key, under the thing's: realtime refetches every list like this
  // when any message, page or task changes, as any of them may link here.
  const query = useQuery({ ...thingQuery(kind, id), queryKey: ['things', kind, id, 'references'] })
  const references = (query.data?.links ?? []).filter((link) => link.kind === 'about' && link.direction === 'incoming')
  if (references.length === 0) return null
  return (
    <Section id={`referenced-in-${id}`} title="Referenced in">
      <RowList label="Referenced in">
        {references.map(({ other }) => (
          <Row
            key={`${other.kind}:${other.id}`}
            leading={<Icon name={thingIcon(other.kind)} size={16} />}
            meta={
              <span className="aw-references__kind">
                {KIND_WORDS[other.kind] ?? 'Link'}
                {other.status ? ` · ${other.status}` : ''}
              </span>
            }
            title={<Link to={other.href}>{other.title}</Link>}
          />
        ))}
      </RowList>
    </Section>
  )
}
