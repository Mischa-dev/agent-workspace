import type { RealtimeEnvelope } from './realtimeModel'

/**
 * Runs that just finished, for whoever wants to say so (W1.24: the toast
 * with Open and its pull request). The event carries ids and the state
 * only; a listener reads the run itself, with its reader's access.
 */
type Listener = (runId: string) => void

const listeners = new Set<Listener>()

export function onRunFinished(listener: Listener) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

export function announceRunFinished(envelope: RealtimeEnvelope) {
  if (envelope.type !== 'run.changed' || envelope.payload.change !== 'finished') return
  const runId = envelope.payload.runId ?? envelope.payload.run_id
  if (typeof runId !== 'string' || !runId) return
  for (const listener of listeners) listener(runId)
}
