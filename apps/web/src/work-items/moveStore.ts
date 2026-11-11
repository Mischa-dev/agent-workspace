import type { MoveDone, MoveResolution } from './moveApi'

/**
 * The memory behind moveRecovery.ts (AWD-102), kept apart so that signing in
 * or out can forget every move at once without loading the move code itself:
 * this is all the first page load carries. Sending and checking moves stays
 * in moveRecovery.ts, which only the Move dialog loads.
 */
export type MoveBody = { targetProjectId: string; snapshot: string; resolutions: MoveResolution[]; leaveUnresolved: boolean }

export type PendingMove = {
  actorId: string
  targetProjectId: string
  targetProjectKey: string
  body: MoveBody
  key: string
  state:
    | { kind: 'sending'; retry: boolean }
    /** No answer, or one that doesn't say whether it happened. */
    | { kind: 'unknown'; message: string }
    | { kind: 'done'; done: MoveDone }
    /** A clear answer that it didn't happen; nothing moved. */
    | { kind: 'refused'; status: number; message: string; retry: boolean }
}

/** By `${actorId}:${projectId}:${taskId}`. */
export const pending = new Map<string, PendingMove>()
export const listeners = new Set<() => void>()
/** Moves on at every clear, so an answer sent before it can't write back. */
export let generation = 0

export function changed() {
  for (const listener of listeners) listener()
}

/** Forgets every move, and stops answers still on their way from writing back: on sign-in, sign-out, and in tests. */
export function clearPendingMoves() {
  generation += 1
  if (pending.size === 0) return
  pending.clear()
  changed()
}
