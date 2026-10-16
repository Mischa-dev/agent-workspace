import { useMutation, useQueryClient, type QueryClient, type QueryKey } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { showToast, type ToastInput } from '../ui'

/**
 * One way to change things from the web: update the screen at once, say what
 * happened in a toast (with Undo where it can be undone), and on failure put
 * the screen back and explain what went wrong. Nothing tells you to reload.
 */

/** A plain sentence for a failed request, and whether fresh data would fix it. */
export function describeError(error: unknown): { message: string; stale: boolean } {
  if (isApiError(error)) {
    if (error.status === 409 || error.code.includes('conflict')) {
      return { message: 'Someone changed this first. You’re now looking at the latest version.', stale: true }
    }
    if (error.status === 403) return { message: 'You don’t have permission to do that here.', stale: false }
    if (error.status === 404) return { message: 'It’s no longer there. It may have been removed.', stale: true }
    if (error.status === 429) return { message: 'That’s a lot at once. Wait a moment and try again.', stale: false }
    if (error.status >= 500) return { message: 'The server had a problem. Try again in a moment.', stale: false }
    return { message: error.message, stale: false }
  }
  if (error instanceof TypeError || (typeof navigator !== 'undefined' && navigator.onLine === false)) {
    return { message: 'You seem to be offline. Check your connection and try again.', stale: false }
  }
  return { message: error instanceof Error && error.message ? error.message : 'Something went wrong. Try again.', stale: false }
}

/** Show a failure as a toast. Returns whether the data on screen is out of date. */
export function toastError(title: string, error: unknown) {
  const { message, stale } = describeError(error)
  showToast({ tone: 'danger', title, description: message })
  return stale
}

type Snapshot = { queryKey: QueryKey; data: unknown }[]

/** Change cached data in place and return how to put it back. */
export function patchQueries<T>(queryClient: QueryClient, queryKey: QueryKey, update: (current: T) => T): () => void {
  const snapshot: Snapshot = queryClient.getQueriesData({ queryKey }).map(([key, data]) => ({ queryKey: key, data }))
  queryClient.setQueriesData({ queryKey }, (current: unknown) => (current === undefined ? current : update(current as T)))
  return () => {
    for (const entry of snapshot) queryClient.setQueryData(entry.queryKey, entry.data)
  }
}

export type InstantMutationOptions<TInput, TResult> = {
  mutationFn: (input: TInput) => Promise<TResult>
  /** Update the cache before the server answers; return how to undo it. */
  optimistic?: (queryClient: QueryClient, input: TInput) => (() => void) | void
  /** Queries to refresh after it settles, success or not. */
  invalidate?: (input: TInput, result: TResult | undefined) => QueryKey[]
  /** What to say when it worked; add `onUndo` to offer Undo. */
  success?: (result: TResult, input: TInput) => ToastInput | null
  /** The toast title when it fails, such as "Couldn’t archive #design". */
  errorTitle: string | ((input: TInput) => string)
  onSuccess?: (result: TResult, input: TInput) => void
}

export function useInstantMutation<TInput, TResult>(options: InstantMutationOptions<TInput, TResult>) {
  const queryClient = useQueryClient()
  return useMutation<TResult, unknown, TInput, { rollback?: () => void }>({
    mutationFn: options.mutationFn,
    onMutate: async (input) => {
      if (!options.optimistic) return {}
      // Keep an in-flight refetch from overwriting the instant change.
      const keys = options.invalidate?.(input, undefined) ?? []
      await Promise.all(keys.map((queryKey) => queryClient.cancelQueries({ queryKey })))
      return { rollback: options.optimistic(queryClient, input) ?? undefined }
    },
    onError: (error, input, context) => {
      context?.rollback?.()
      const title = typeof options.errorTitle === 'function' ? options.errorTitle(input) : options.errorTitle
      toastError(title, error)
    },
    onSuccess: (result, input) => {
      const toast = options.success?.(result, input)
      if (toast) showToast(toast)
      options.onSuccess?.(result, input)
    },
    // Stays pending until the refetch lands, so controls don't re-enable on
    // capabilities that still describe the old state.
    onSettled: async (result, _error, input) => {
      await Promise.all(
        (options.invalidate?.(input, result) ?? []).map((queryKey) => queryClient.invalidateQueries({ queryKey })),
      )
    },
  })
}
