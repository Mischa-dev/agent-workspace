import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { requestJson } from '../../api/client'
import { isApiError } from '../../api/errors'
import type { Piece, Thing, ThingDecl } from '../../modules/contract'
import { PieceView } from '../../modules/pieces/Pieces'
import { Button, Icon, ListSkeleton, countLabel } from '../../ui'
import { listWorkItems } from '../../work-items/api'
import type { WorkItemCard } from '../../work-items/types'
import { moduleEmbedTarget } from './moduleEmbed'
import './moduleEmbed.css'

/** The most tasks a task list embed shows. */
const TASKS = 10

/**
 * A module's embed in a page (AWD-53): one thing, or a project's tasks, drawn
 * with the piece its module's manifest names. It loads as whoever reads the
 * page, so it shows only what they may see; any other address stays a link.
 */
export default function ModuleEmbed({ href, label }: { href: string; label: string }) {
  const target = moduleEmbedTarget(href)
  if (!target) return <EmbedLink href={href} label={label} />
  if (target.kind === 'thing') return <ThingEmbed decl={target.decl} href={href} id={target.id} kind={target.thing} label={label} piece={target.embed.piece} />
  return <TasksEmbed decl={target.decl} filters={target.filters} href={href} label={label} piece={target.embed.piece} projectId={target.projectId} />
}

function EmbedLink({ href, label }: { href: string; label: string }) {
  return (
    <Link className="aw-embed__link" to={href}>
      <Icon name="link" size={16} />
      <span>{label || href}</span>
    </Link>
  )
}

function Problem({ error, href, label, onRetry }: { error: unknown; href: string; label: string; onRetry: () => void }) {
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

/** The piece's fields that the things have values for, so a card shows no empty rows. */
function filled(piece: Piece, things: Thing[]): Piece {
  const has = (id: string) =>
    things.some((thing) => (id === 'status' ? thing.status : id === 'owner' ? thing.owner : thing.fields?.[id]) != null)
  return { ...piece, fields: (piece.fields ?? []).filter(has) }
}

function ThingEmbed({ decl, href, id, kind, label, piece }: { decl: ThingDecl; href: string; id: string; kind: string; label: string; piece: Piece }) {
  const query = useQuery({
    queryKey: ['things', kind, id],
    queryFn: () => requestJson<{ thing: Thing }>(`/api/v1/things/${encodeURIComponent(kind)}/${encodeURIComponent(id)}`),
    retry: (count, error) => !(isApiError(error) && error.status < 500) && count < 2,
  })
  const name = label || decl.one
  if (query.isPending) return <ListSkeleton label={`Loading ${name}`} rows={1} />
  if (!query.data) return <Problem error={query.error} href={href} label={name} onRetry={() => void query.refetch()} />
  const things = [query.data.thing]
  return (
    <div className="aw-membed aw-membed--thing">
      <PieceView decl={decl} label={name} piece={filled(piece, things)} things={things} />
    </div>
  )
}

function taskThing(card: WorkItemCard): Thing {
  return {
    id: card.id,
    kind: 'task',
    module: 'tasks',
    title: card.title,
    status: card.status.name,
    ...(card.assignee ? { owner: { id: card.assignee.actorId, name: card.assignee.displayName } } : {}),
    projectId: card.projectId,
    href: `/projects/${encodeURIComponent(card.projectId)}/work-items/${encodeURIComponent(card.id)}`,
    fields: { key: card.key, priority: card.priority, dueDate: card.dueDate, estimate: card.estimate },
  }
}

function TasksEmbed({
  decl,
  filters,
  href,
  label,
  piece,
  projectId,
}: {
  decl: ThingDecl
  filters: Record<string, string>
  href: string
  label: string
  piece: Piece
  projectId: string
}) {
  const query = useQuery({
    queryKey: ['work-items', projectId, 'embed', filters],
    queryFn: () => listWorkItems(projectId, { ...filters, limit: TASKS }),
    retry: (count, error) => !(isApiError(error) && error.status < 500) && count < 2,
  })
  const name = label || decl.many
  if (query.isPending) return <ListSkeleton label={`Loading ${name}`} rows={3} />
  if (!query.data) return <Problem error={query.error} href={href} label={name} onRetry={() => void query.refetch()} />
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
