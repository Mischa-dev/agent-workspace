import { createContext, useContext } from 'react'

export type ConnectionStatus = 'connecting' | 'live' | 'retrying'

export type RealtimeContextValue = {
  connection: ConnectionStatus
  streams: Record<string, Record<string, string>>
}

export const RealtimeContext = createContext<RealtimeContextValue | null>(null)

export function useRealtime() {
  const realtime = useContext(RealtimeContext)
  if (!realtime) {
    throw new Error('Realtime state rendered outside an authenticated session.')
  }
  return realtime
}

export function useConversationRealtime(conversationId: string) {
  const realtime = useRealtime()
  return {
    connection: realtime.connection,
    streaming: realtime.streams[conversationId] ?? {},
  }
}
