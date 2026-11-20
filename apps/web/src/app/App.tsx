import { useEffect, useState } from 'react'
import { onlineManager, QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createBrowserRouter, RouterProvider } from 'react-router-dom'

import { offlineOwner } from '../data/offlinePointer'
import { ThemeProvider } from '../theme/ThemeProvider'
import { appRoutes } from './routes'

export { AuthenticatedRouteBoundary } from './boundaries'

function createQueryClient() {
  return new QueryClient({
    defaultOptions: {
      queries: {
        refetchOnWindowFocus: true,
        retry: 1,
      },
      mutations: {
        retry: false,
      },
    },
  })
}

/** The app: its addresses are the route table in `routes.tsx`. */
/** The longest the first paint waits for the offline copy to load. */
const RESTORE_WAIT_MS = 1500

export function App() {
  const [queryClient] = useState(createQueryClient)
  const [router] = useState(() => createBrowserRouter(appRoutes))
  // With reading offline on (W1.31), show what this device kept before
  // anything asks the network; without it, start at once.
  const [restored, setRestored] = useState(() => !offlineOwner())
  useEffect(() => {
    // The browser knows at start whether it's offline; queries then wait instead of failing.
    if (typeof navigator !== 'undefined' && navigator.onLine === false) onlineManager.setOnline(false)
    if (restored) return
    let done = false
    const finish = () => {
      if (done) return
      done = true
      setRestored(true)
    }
    const timer = window.setTimeout(finish, RESTORE_WAIT_MS)
    void import('../data/persist')
      .then((persist) => persist.restoreOffline(queryClient))
      .catch(() => false)
      .finally(finish)
    return () => window.clearTimeout(timer)
    // Once, at start.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  return (
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        {restored ? <RouterProvider router={router} /> : null}
      </QueryClientProvider>
    </ThemeProvider>
  )
}
