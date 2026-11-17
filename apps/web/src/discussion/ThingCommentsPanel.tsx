import { lazy, Suspense } from 'react'

import type { ThingCommentsProps } from './ThingComments'

const ThingComments = lazy(() => import('./ThingComments'))

/**
 * A thing's comments, loaded on first use. Pages that host it check
 * `shows('thing.comments')` first.
 */
export function ThingCommentsPanel(props: ThingCommentsProps) {
  return (
    <Suspense fallback={null}>
      <ThingComments {...props} />
    </Suspense>
  )
}
