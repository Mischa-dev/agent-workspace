import type { ReactNode } from 'react'

import { RealtimeContext } from './realtimeContext'
import { useRealtimeConnection } from './useRealtimeConnection'

export function RealtimeProvider({ children }: { children: ReactNode }) {
  const value = useRealtimeConnection()
  return (
    <RealtimeContext.Provider value={value}>
      {children}
    </RealtimeContext.Provider>
  )
}
