import { isApiError } from '../api/errors'
import { moveTask, type MoveDone, type MoveResolution } from './moveApi'

/**
 * Confirmed moves in flight, and ones whose answer never arrived (AWD-102).
 *
 * A confirmation is recorded here, with its exact body and key, before it is
 * sent, and sent from here rather than from the dialog, so the dialog can
 * close, reopen or remount (a new version of the task) without losing it or
 * sending a second one. While a move is in flight or its outcome is unknown,
 * the only request that can be sent for that task is that same one again:
 * the server answers it with the move's result if it happened (its receipt),
 * and can't make it happen twice.
 *
 * Only in this tab's memory, for the person signed in when it was sent (by
 * actor ID), never in storage another session or tab could read. Any sign-in
 * or sign-out forgets everything, and a request still in flight from before
 * then can't write its answer back (a session generation guards it). A
 * reload forgets it too, and with it the key, so it can't be resent from the
 * browser; the dialog says so, and the task shows what happened: a moved
 * task is archived with a link to where it went.
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

const pending = new Map<string, PendingMove>()
const listeners = new Set<() => void>()
let generation = 0

const id = (actorId: string, projectId: string, taskId: string) => `${actorId}:${projectId}:${taskId}`

function changed() {
  for (const listener of listeners) listener()
}

export function pendingMove(actorId: string, projectId: string, taskId: string): PendingMove | undefined {
  const move = pending.get(id(actorId, projectId, taskId))
  return move?.actorId === actorId ? move : undefined
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

/** Forgets every move, and stops answers still on their way from writing back: on sign-in, sign-out, and in tests. */
export function clearPendingMoves() {
  generation += 1
  if (pending.size === 0) return
  pending.clear()
  changed()
}

/**
 * How an error answers a move request. On a first attempt any refusal
 * (4xx) is reliable: the move is one transaction, so a refused request wrote
 * nothing. On a retry of a request that may have happened, only a 409 or 422
 * shows it didn't: the server answers a moved task's same request from its
 * receipt before it checks the preview or the choices, so those mean the task
 * was never moved. A 401, 403 or 404 then may just mean access was lost since.
 * No answer, a server failure, or an answer that can't be read (even a 200)
 * says nothing either way.
 */
export function outcomeOf(error: unknown, retry: boolean): 'refused' | 'unknown' {
  if (!isApiError(error) || error.status < 400 || error.status >= 500) return 'unknown'
  if (!retry) return 'refused'
  return error.status === 409 || error.status === 422 ? 'refused' : 'unknown'
}

function messageOf(error: unknown) {
  return isApiError(error) ? error.message : error instanceof Error && error.message ? error.message : 'No answer.'
}

/**
 * Sends a move: a new one (`move` given, recorded before it goes) or the
 * recorded one again. Never two at once for one task.
 */
export async function sendMove(
  actorId: string,
  projectId: string,
  taskId: string,
  move?: Omit<PendingMove, 'actorId' | 'state'>,
): Promise<void> {
  const key = id(actorId, projectId, taskId)
  const current = pending.get(key)
  if (current?.state.kind === 'sending') return
  if (move && current && current.state.kind !== 'refused') return
  const base = move ? { ...move, actorId } : current
  if (!base) return
  const retry = !move
  const started = generation
  const sending: PendingMove = { ...base, actorId, state: { kind: 'sending', retry } }
  pending.set(key, sending)
  changed()
  const settle = (state: PendingMove['state']) => {
    // Signed out or in as someone else since: this answer is no one's now.
    if (generation !== started || pending.get(key) !== sending) return
    pending.set(key, { ...sending, state })
    changed()
  }
  try {
    const done = await moveTask(projectId, taskId, sending.body, sending.key)
    settle({ kind: 'done', done })
  } catch (error) {
    settle(
      outcomeOf(error, retry) === 'refused'
        ? { kind: 'refused', status: isApiError(error) ? error.status : 0, message: messageOf(error), retry }
        : { kind: 'unknown', message: messageOf(error) },
    )
  }
}
