import { useQuery } from '@tanstack/react-query'
import { Navigate, useParams } from 'react-router-dom'

import { thingQuery } from '../modules/references/thingQuery'
import { useShellPage } from '../shell/shellStore'
import { EmptyState, ErrorState, PageSkeleton } from '../ui'
import { isApiError } from '../api/errors'
import { workspaceHref } from './href'

/**
 * `/w/:ws/t/:kind/:id` (W1.14b): a thing's permanent address. The server
 * says where the thing is now (its `href`, which survives a rename or a move
 * to another space) and the address is replaced with it.
 */
export function ThingRedirect() {
  const { ws = '', kind = '', id = '' } = useParams()
  useShellPage({ mainClassName: 'aw-main--page' })
  const thing = useQuery({ ...thingQuery(kind, id), enabled: Boolean(kind && id) })
  const href = thing.data?.thing.href
  if (href) return <Navigate replace to={href.startsWith('/w/') ? href : workspaceHref(ws, href)} />
  return (
    <div className="aw-page" role={thing.isPending ? undefined : 'alert'}>
      {thing.isPending ? (
        <PageSkeleton label="Opening" />
      ) : isApiError(thing.error) && thing.error.status === 404 ? (
        <EmptyState icon="search" title="This isn’t here">
          It may have been deleted, or you may not have access to it.
        </EmptyState>
      ) : (
        <ErrorState error={thing.error} onRetry={() => void thing.refetch()} title="It didn’t open" />
      )}
    </div>
  )
}
