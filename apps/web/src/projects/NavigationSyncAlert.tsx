import { Banner, Button } from '../ui'

/** Shown when the app couldn't remember where you were in a project. Nothing else is affected. */
export function NavigationSyncAlert({ error, retry }: { error: string | null; retry: () => void }) {
  if (!error) return null
  return (
    <div className="aw-sync-alert">
      <Banner
        action={
          <Button onClick={retry} size="sm" variant="ghost">
            Try again
          </Button>
        }
        role="alert"
        title="Your place here wasn’t saved."
        tone="warning"
      >
        {error}
      </Banner>
    </div>
  )
}
