import { lazy, Suspense, type ComponentProps } from 'react'

const LaunchDialog = lazy(() => import('./LaunchDialog').then((module) => ({ default: module.LaunchDialog })))

/** The launch dialog, loaded when it first opens so task pages stay light. */
export function LazyLaunchDialog(props: ComponentProps<typeof LaunchDialog>) {
  return (
    <Suspense fallback={null}>
      <LaunchDialog {...props} />
    </Suspense>
  )
}
