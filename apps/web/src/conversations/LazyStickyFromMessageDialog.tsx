import { Suspense, lazy, type ComponentProps } from 'react'

const StickyFromMessageDialog = lazy(() => import('./StickyFromMessageDialog'))

export function LazyStickyFromMessageDialog(props: ComponentProps<typeof StickyFromMessageDialog>) {
  return (
    <Suspense fallback={null}>
      <StickyFromMessageDialog {...props} />
    </Suspense>
  )
}
