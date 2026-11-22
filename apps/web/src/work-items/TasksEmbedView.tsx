import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { spaceHref } from '../app/href'
import { queryKeys } from '../api/queryKeys'
import type { Thing } from '../modules/contract'
import type { EmbedViewProps } from '../modules/define'
import { PieceView } from '../pieces/Pieces'
import { EmbedLink, EmbedProblem } from '../pieces/embeds/parts'
import { declOf, embedPiece, filled, retryServerErrors } from '../pieces/embeds/embedParts'
import { Icon, ListSkeleton, countLabel } from '../ui'
import { listWorkItems } from './api'
import { listQuery, readListState } from './listModel'
import type { WorkItemCard } from './types'

/** The most tasks a task list embed shows. */
const TASKS = 10

function taskThing(card: WorkItemCard): Thing {
  return {
    id: card.id,
    kind: 'task',
    module: 'tasks',
    title: card.title,
    status: card.status.name,
    ...(card.assignee ? { owner: { id: card.assignee.actorId, name: card.assignee.displayName } } : {}),
    projectId: card.projectId,
    href: spaceHref(card.projectId, `/work-items/${encodeURIComponent(card.id)}`),
    fields: { key: card.key, priority: card.priority, dueDate: card.dueDate, estimate: card.estimate },
  }
}

/**
 * A space's tasks in a page (W1.26), from the task list's own address: its
 * filters and sort go to the server, and its view (list or board) picks the
 * piece the tasks manifest declares.
 */
export default function TasksEmbedView({ href, label, params }: EmbedViewProps) {
  const projectId = params.projectId ?? ''
  const search = new URLSearchParams(params.query ?? '')
  const state = readListState(search)
  const board = state.view === 'board' || search.get('layout') === 'board'
  const filters = listQuery(state)
  const decl = declOf('task')
  const piece = embedPiece('task', board ? 'board' : 'list')
  const query = useQuery({
    // Under the space's task queries, so task changes refetch it.
    queryKey: queryKeys.workItemEmbed(projectId, filters),
    queryFn: () => listWorkItems(projectId, { ...filters, limit: TASKS }),
    enabled: Boolean(decl && piece && projectId),
    retry: retryServerErrors,
  })
  if (!decl || !piece || !projectId) return <EmbedLink href={href} label={label} />
  const name = label || decl.many
  if (query.isPending) return <ListSkeleton label={`Loading ${name}`} rows={3} />
  if (!query.data) return <EmbedProblem error={query.error} href={href} label={name} onRetry={() => void query.refetch()} />
  const things = query.data.items.slice(0, TASKS).map(taskThing)
  const total = query.data.total
  return (
    <div className="aw-membed">
      <div className="aw-membed__head">
        <Icon name="list" size={16} />
        <Link className="aw-membed__name" to={href}>
          {name}
        </Link>
        <span className="aw-membed__count">{countLabel(total, 'task')}</span>
      </div>
      <PieceView decl={decl} label={name} piece={filled(piece, things)} things={things} />
      {total > things.length ? (
        <Link className="aw-membed__more" to={href}>
          See all {total}
        </Link>
      ) : null}
    </div>
  )
}
