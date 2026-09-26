import { Button, ErrorState, MessageSkeleton } from '../ui'

/** Shown inside the shell while a Project page loads. */
export function ProjectPageLoading({ label = 'Loading Project' }: { label?: string }) {
  return (
    <div aria-busy="true" className="aw-page">
      <MessageSkeleton label={label} rows={4} />
    </div>
  )
}

/** Shown when a Project page couldn't load, or the Project is gone or out of reach. */
export function ProjectPageError({ onRetry }: { onRetry: () => void }) {
  return (
    <div className="aw-page">
      <ErrorState action={<Button onClick={onRetry}>Try again</Button>} title="This Project isn’t available">
        It may have been archived, or your access may have changed. If your connection dropped, try again.
      </ErrorState>
    </div>
  )
}
