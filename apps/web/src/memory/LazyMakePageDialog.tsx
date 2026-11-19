import { lazy, Suspense, type ComponentProps } from 'react'

const MakePageDialog = lazy(() => import('./MakePageDialog').then((module) => ({ default: module.MakePageDialog })))

/** Make a page, loaded the first time someone opens it so conversations and pages stay light. */
export function LazyMakePageDialog(props: ComponentProps<typeof MakePageDialog>) {
  return (
    <Suspense fallback={null}>
      <MakePageDialog {...props} />
    </Suspense>
  )
}
