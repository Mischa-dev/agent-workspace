import { lazy, Suspense } from 'react'
import { Link } from 'react-router-dom'

import { Icon, ListSkeleton } from '../../ui'
import { embedTarget } from './embedModel'
import './embeds.css'

const CollectionEmbed = lazy(() => import('../../collections/CollectionEmbed'))

/**
 * Something of the app's shown live in a page (an embed block): a
 * collection's view, or else a link to the path. `framed` draws its box; the
 * page editor keeps the box itself, around the block it can select.
 */
export function PageEmbed({ framed = true, href, label }: { framed?: boolean; href: string; label: string }) {
  const target = embedTarget(href)
  const content =
    target.kind === 'collection' ? (
      <Suspense fallback={<ListSkeleton label={`Loading ${label || 'the collection'}`} rows={3} />}>
        <CollectionEmbed collectionId={target.collectionId} label={label} projectId={target.projectId} viewId={target.viewId} />
      </Suspense>
    ) : (
      <Link className="aw-embed__link" to={href}>
        <Icon name="link" size={16} />
        <span>{label || href}</span>
      </Link>
    )
  return framed ? <div className="aw-embed">{content}</div> : content
}
