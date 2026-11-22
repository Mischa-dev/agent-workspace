import { useQuery } from '@tanstack/react-query'

import { requestJson } from '../../api/client'
import { queryKeys } from '../../api/queryKeys'
import type { Thing } from '../../modules/contract'
import type { EmbedViewProps } from '../../modules/define'
import { ListSkeleton } from '../../ui'
import { PieceView } from '../Pieces'
import { EmbedLink, EmbedProblem } from './parts'
import { declOf, embedPiece, filled, retryServerErrors } from './embedParts'

/**
 * One thing in a page (W1.26): what a module registers for a task, a page, a
 * whiteboard or a thread, drawn with the card its manifest declares. It loads
 * as whoever reads the page, so it shows only what they may see.
 */
export default function ThingEmbedView({ href, label, params }: EmbedViewProps) {
  const kind = params.kind ?? ''
  const id = params.id ?? ''
  const decl = declOf(kind)
  const piece = embedPiece(kind, 'card')
  const query = useQuery({
    queryKey: queryKeys.thing(kind, id),
    queryFn: () => requestJson<{ thing: Thing }>(`/api/v1/things/${encodeURIComponent(kind)}/${encodeURIComponent(id)}`),
    enabled: Boolean(decl && piece && id),
    retry: retryServerErrors,
  })
  if (!decl || !piece || !id) return <EmbedLink href={href} label={label} />
  const name = label || decl.one
  if (query.isPending) return <ListSkeleton label={`Loading ${name}`} rows={1} />
  if (!query.data) return <EmbedProblem error={query.error} href={href} label={name} onRetry={() => void query.refetch()} />
  const things = [query.data.thing]
  return (
    <div className="aw-membed aw-membed--thing">
      <PieceView decl={decl} label={name} piece={filled(piece, things)} things={things} />
    </div>
  )
}
