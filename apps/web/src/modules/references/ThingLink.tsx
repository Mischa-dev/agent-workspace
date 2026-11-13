import { lazy, Suspense, type ReactNode } from 'react'
import { Link } from 'react-router-dom'

import type { ThingAddress } from './thingLinks'

// The chip loads on its own, so a page with no link to a thing never
// downloads it; until it does, the link is the one that was written.
const ThingChip = lazy(() => import('./ThingChip'))

export function ThingLink({ address, bare, children }: { address: ThingAddress; bare: boolean; children: ReactNode }) {
  return (
    <Suspense fallback={<Link to={address.path}>{children}</Link>}>
      <ThingChip address={address} bare={bare}>
        {children}
      </ThingChip>
    </Suspense>
  )
}
