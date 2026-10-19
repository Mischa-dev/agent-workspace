import { isApiError } from '../api/errors'
import { ButtonLink, ErrorState, PageSkeleton } from '../ui'

/** Shown inside the shell while a project page loads. */
export function ProjectPageLoading({ label = 'Loading the project' }: { label?: string }) {
  return (
    <div aria-busy="true" className="aw-page">
      <PageSkeleton label={label} />
    </div>
  )
}

/**
 * Shown when a project page couldn't load. Given the error, it tells a project
 * that's gone or out of reach (with a way Home) from one that just didn't load
 * (with Try again).
 */
export function ProjectPageError({ error, onRetry }: { error?: unknown; onRetry: () => void }) {
  const known = error !== undefined
  const gone = isApiError(error) && (error.status === 403 || error.status === 404)
  return (
    <div className="aw-page">
      {gone ? (
        <ErrorState
          action={
            <ButtonLink to="/home" variant="primary">
              Go to Home
            </ButtonLink>
          }
          title="This project isn’t available"
        >
          It may have been archived, or your access may have changed.
        </ErrorState>
      ) : known ? (
        <ErrorState error={error} onRetry={onRetry} title="This project didn’t load" />
      ) : (
        <ErrorState onRetry={onRetry} title="This project isn’t available">
          It may have been archived, or your access may have changed. If your connection dropped, try again.
        </ErrorState>
      )}
    </div>
  )
}
