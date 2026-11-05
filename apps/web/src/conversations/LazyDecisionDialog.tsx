import { lazy, Suspense, type ComponentProps } from 'react'

const DecisionDialog = lazy(() => import('./DecisionDialog').then((module) => ({ default: module.DecisionDialog })))

/** Save as decision, loaded the first time someone opens it so conversations stay light. */
export function LazyDecisionDialog(props: ComponentProps<typeof DecisionDialog>) {
  return (
    <Suspense fallback={null}>
      <DecisionDialog {...props} />
    </Suspense>
  )
}
