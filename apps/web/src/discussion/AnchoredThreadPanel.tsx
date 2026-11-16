import { lazy, Suspense } from 'react'

import type { AnchoredThreadProps } from './AnchoredThread'

const AnchoredThread = lazy(() => import('./AnchoredThread'))

/**
 * The discussion about one place in a thing, loaded on first use. Pages
 * that host it check `shows('thing.comments')` first.
 */
export function AnchoredThreadPanel(props: AnchoredThreadProps) {
  return (
    <Suspense fallback={null}>
      <AnchoredThread {...props} />
    </Suspense>
  )
}
