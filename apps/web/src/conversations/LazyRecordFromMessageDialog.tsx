import { Suspense, lazy, type ComponentProps } from 'react'

const RecordFromMessageDialog = lazy(() => import('./RecordFromMessageDialog'))

export function LazyRecordFromMessageDialog(props: ComponentProps<typeof RecordFromMessageDialog>) {
  return (
    <Suspense fallback={null}>
      <RecordFromMessageDialog {...props} />
    </Suspense>
  )
}
