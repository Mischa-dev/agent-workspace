import { useNavigate } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { Button, EmptyState } from '../ui'

/** An address that isn't part of the app. Loaded only when it's needed. */
export function NotFoundPage() {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page" role="alert">
        <EmptyState
          action={
            <Button onClick={() => navigate('/home')} variant="primary">
              Go to Home
            </Button>
          }
          icon="search"
          title="Page not found"
        >
          That address isn’t part of this workspace. It may have moved, or the link may be old.
        </EmptyState>
      </div>
    </AppShell>
  )
}
