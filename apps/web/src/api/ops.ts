// The op client (W1.4): every new call the web app makes goes through here,
// over the descriptors the server generates (api/generated). A write gets a
// request id (UUIDv7) once and keeps it across retries, so a retried write
// is answered once; a refusal is an `ApiError` whose code is the operation's
// outcome, and `opErrorSentence` (ui/errorText.ts) turns each outcome into
// a plain sentence.
// Hand-written `requestJson` calls are on a ratchet (api/handwritten.test.ts).

import { useMutation, type QueryClient, type QueryKey, useQueryClient } from '@tanstack/react-query'
import { useRef } from 'react'

import { toastError } from '../app/toastError'
import { things, type Thing, type ThingRef } from '../data/things'
import { isApiError } from './errors'
import { callOp as callGenerated, type CallOptions } from './generated/client'
import type { OpName, Operations } from './generated/operations'

export type OpInput<N extends OpName> = Operations[N]['input']
export type OpOutput<N extends OpName> = Operations[N]['output']

/** A UUIDv7: time-ordered, so request ids sort by when they were made. */
export function newRequestId(now: number = Date.now()): string {
  const bytes = new Uint8Array(16)
  crypto.getRandomValues(bytes)
  let time = now
  for (let index = 5; index >= 0; index -= 1) {
    bytes[index] = time % 256
    time = Math.floor(time / 256)
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x70
  bytes[8] = (bytes[8] & 0x3f) | 0x80
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}

export type OpCallOptions = CallOptions & {
  /** The version the change was made against; a newer one is a conflict. */
  expectedVersion?: number
  /** Retries after a dropped connection or a busy server, with the same request id (default 1). */
  retries?: number
}

/** The outcomes worth trying again unchanged. */
const RETRIED: ReadonlySet<string> = new Set(['temporarily_unavailable', 'rate_limited', 'network'])

function retryable(error: unknown) {
  if (isApiError(error)) return RETRIED.has(error.code) || error.status >= 502
  return error instanceof TypeError
}

/**
 * Run an operation. One request id for every try, so the server answers a
 * retried write once; `expectedVersion` goes in the input.
 */
export async function callOp<N extends OpName>(
  name: N,
  input: OpInput<N>,
  options: OpCallOptions = {},
): Promise<OpOutput<N>> {
  const { expectedVersion, retries = 1, ...call } = options
  const requestId = call.requestId ?? newRequestId()
  const body = (
    expectedVersion === undefined ? input : { ...(input as object), expectedVersion }
  ) as OpInput<N>
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await callGenerated(name, body, { ...call, requestId })
    } catch (error) {
      if (attempt >= retries || !retryable(error) || call.signal?.aborted) throw error
    }
  }
}

/**
 * The review request a team's guard holds a write under, when it does
 * (A2.27): the write didn't happen yet. Once a reviewer allows it, the same
 * call made again goes through.
 */
export function heldForReview(value: unknown): string | null {
  if (!value || typeof value !== 'object') return null
  const held = value as { status?: unknown; requestId?: unknown }
  return held.status === 'pending_review' && typeof held.requestId === 'string' ? held.requestId : null
}

/**
 * An operation's answer once its command has run: a write a review guard
 * held instead throws, so nothing reports it done. `what` names the write,
 * as a sentence's subject ("Cancelling it").
 */
export function ranOrThrow<T>(value: T, what: string): T {
  if (heldForReview(value)) throw new Error(`${what} waits for a reviewer to allow it. Once they have, try again.`)
  return value
}

/** The query key of an operation's answer: workspace, name, input. */
export function opKey<N extends OpName>(name: N, input: OpInput<N>, workspaceId?: string): QueryKey {
  return ['op', workspaceId ?? null, name, input]
}

/** The prefix every query of this operation shares, for invalidating them all. */
export function opPrefix(name: OpName, workspaceId?: string): QueryKey {
  return ['op', workspaceId ?? null, name]
}

/** A read as a query: `useQuery(opQuery('decision_list', { spaceId }))`. */
export function opQuery<N extends OpName>(name: N, input: OpInput<N>, options: { workspaceId?: string } = {}) {
  return {
    queryKey: opKey(name, input, options.workspaceId),
    queryFn: async ({ signal }: { signal?: AbortSignal }) => {
      const value = await callOp(name, input, { workspaceId: options.workspaceId, signal, retries: 0 })
      // Every thing in the answer goes to the store (W1.5).
      things.normalise(value)
      return value
    },
  }
}

/** Whether a refusal says the thing changed since it was read. */
export function isVersionConflict(error: unknown): boolean {
  if (!isApiError(error) || error.code !== 'conflict') return false
  const details = error.details as { reason?: unknown; code?: unknown } | null | undefined
  return details?.reason === 'version' || details?.code === 'version'
}

/** A list read: the same as `opQuery`, named for what it reads. */
export const opList = opQuery

/**
 * A write as a mutation. Each `mutate` gets a request id that its retries
 * (React Query's or `callOp`'s) keep; `invalidates` names the reads it
 * changes.
 */
export function useOp<N extends OpName>(
  name: N,
  options: {
    workspaceId?: string
    invalidates?: OpName[]
    onSuccess?: (value: OpOutput<N>) => void
    /** The change to show at once (W1.5): an overlay on one thing until the write settles. */
    optimistic?: (input: OpInput<N>) => { ref: ThingRef; patch: Record<string, unknown> } | null
    /** Read one thing again, after a version conflict. */
    refetch?: (ref: ThingRef) => Promise<Thing | null>
  } = {},
) {
  const queryClient = useQueryClient()
  const pending = useRef<string | null>(null)
  return useMutation({
    onMutate: (input: OpInput<N> & { expectedVersion?: number }) => {
      const shown = options.optimistic?.(input)
      return shown ? { ref: shown.ref, overlay: things.overlay(shown.ref, shown.patch) } : {}
    },
    onError: (error, _input, context) => {
      context?.overlay?.rollback()
      // Someone changed it first: show theirs, and say so.
      if (context?.ref && isVersionConflict(error)) {
        const ref = context.ref
        void options.refetch?.(ref).then((thing) => {
          if (thing) things.put(thing)
        })
        toastError('Someone changed this first', error)
      }
    },
    mutationFn: async (input: OpInput<N> & { expectedVersion?: number }) => {
      pending.current ??= newRequestId()
      const { expectedVersion, ...rest } = input as OpInput<N> & { expectedVersion?: number }
      return callOp(name, rest as OpInput<N>, {
        workspaceId: options.workspaceId,
        requestId: pending.current,
        expectedVersion,
      })
    },
    onSettled: () => {
      pending.current = null
    },
    onSuccess: async (value, _input, context) => {
      things.normalise(value)
      context?.overlay?.settle()
      await invalidateOps(queryClient, options.invalidates ?? [], options.workspaceId)
      options.onSuccess?.(value)
    },
  })
}

export async function invalidateOps(queryClient: QueryClient, names: OpName[], workspaceId?: string) {
  await Promise.all(names.map((name) => queryClient.invalidateQueries({ queryKey: opPrefix(name, workspaceId) })))
}

export { OUTCOME_SENTENCES, opErrorSentence } from '../ui/errorText'
