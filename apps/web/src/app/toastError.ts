import { describeError, showToast } from '../ui'

/**
 * Show a failure as a toast. Returns whether the data on screen is out of
 * date. Its own module so the shell can use it without the rest of
 * mutations.ts on the first page load.
 */
export function toastError(title: string, error: unknown) {
  const { message, stale } = describeError(error)
  showToast({ tone: 'danger', title, description: message })
  return stale
}
