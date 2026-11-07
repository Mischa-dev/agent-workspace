import type { MoveResolution } from './moveApi'

/**
 * A move whose answer never arrived (AWD-102): the network failed, or the
 * server failed after it may have committed. Until it's settled, the only
 * thing to send is that very request with its key: the server answers it
 * with the move's result if it happened, and nothing else can be sent in its
 * place.
 *
 * Kept only in this tab's memory, for the person signed in when it was sent
 * (by their actor ID, and cleared whenever anyone signs in or out), never in
 * storage another session or tab could read. Closing and reopening the
 * dialog, or leaving the task, finds it again; a reload or a sign-out loses
 * it, and with it the key, so it can't be resent from the browser. The
 * dialog says so, and the task itself shows what happened: a moved task is
 * archived with a link to where it went, and one that didn't move can be
 * moved again from a fresh preview, which can't move it twice.
 */
export type PendingMove = {
  actorId: string
  targetProjectId: string
  targetProjectKey: string
  body: { targetProjectId: string; snapshot: string; resolutions: MoveResolution[]; leaveUnresolved: boolean }
  key: string
}

const pending = new Map<string, PendingMove>()
const listeners = new Set<() => void>()

const id = (actorId: string, projectId: string, taskId: string) => `${actorId}:${projectId}:${taskId}`

function changed() {
  for (const listener of listeners) listener()
}

export function pendingMove(actorId: string, projectId: string, taskId: string): PendingMove | undefined {
  const move = pending.get(id(actorId, projectId, taskId))
  return move?.actorId === actorId ? move : undefined
}

export function rememberMove(projectId: string, taskId: string, move: PendingMove) {
  pending.set(id(move.actorId, projectId, taskId), move)
  changed()
}

export function settleMove(actorId: string, projectId: string, taskId: string) {
  if (pending.delete(id(actorId, projectId, taskId))) changed()
}

export function subscribeMoves(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** Forgets every pending move: on sign-in, sign-out, and in tests. */
export function clearPendingMoves() {
  if (pending.size === 0) return
  pending.clear()
  changed()
}
