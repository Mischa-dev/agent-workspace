import type { MoveResolution } from './moveApi'

/**
 * A move whose answer never arrived (AWD-102): the network failed, or the
 * server failed after it may have committed. Until it's settled, the only
 * thing to send is the very same request, with the same key, so the server
 * returns the move's result if it happened and nothing else can be sent in
 * its place. Kept in this tab's memory only, by task, so closing and
 * reopening the dialog (or leaving the task) finds it again; a reload starts
 * fresh, which is safe: the server's receipt answers the same key whenever
 * it comes.
 */
export type PendingMove = {
  targetProjectId: string
  targetProjectKey: string
  body: { targetProjectId: string; snapshot: string; resolutions: MoveResolution[]; leaveUnresolved: boolean }
  key: string
}

const pending = new Map<string, PendingMove>()
const listeners = new Set<() => void>()

const id = (projectId: string, taskId: string) => `${projectId}:${taskId}`

export function pendingMove(projectId: string, taskId: string): PendingMove | undefined {
  return pending.get(id(projectId, taskId))
}

export function rememberMove(projectId: string, taskId: string, move: PendingMove) {
  pending.set(id(projectId, taskId), move)
  for (const listener of listeners) listener()
}

export function settleMove(projectId: string, taskId: string) {
  if (pending.delete(id(projectId, taskId))) for (const listener of listeners) listener()
}

export function subscribeMoves(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** Test support: forget every pending move. */
export function clearPendingMoves() {
  pending.clear()
}
