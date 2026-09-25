import { Banner, Button } from '../ui'

/** Shown when the app couldn't remember where you were in a Project. Nothing else is affected. */
export function NavigationSyncAlert({ error, retry }: { error: string | null; retry: () => void }) {
  if (!error) return null
  return (
    <div className="aw-sync-alert">
      <Banner
        action={
          <Button onClick={retry} size="sm" variant="ghost">
            Try saving again
          </Button>
        }
        role="alert"
        title="Couldn’t remember your place here."
        tone="warning"
      >
        {error}
      </Banner>
    </div>
  )
}
