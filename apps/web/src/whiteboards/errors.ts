import { isApiError } from '../api/errors'

/** The API's own words for what went wrong, or the fallback. */
export function errorText(reason: unknown, fallback: string) {
  return isApiError(reason) || reason instanceof Error ? reason.message : fallback
}
