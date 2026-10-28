import { useMutation, useQueryClient, type QueryClient, type QueryKey } from '@tanstack/react-query'

import { describeError, showToast, type ToastInput } from '../ui'
import { toastError } from './toastError'

export { describeError, toastError }

/**
 * One way to change things from the web: update the screen at once, say what
 * happened in a toast (with Undo where it can be undone), and on failure put
 * the screen back and explain what went wrong. Nothing tells you to reload.
 */

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
