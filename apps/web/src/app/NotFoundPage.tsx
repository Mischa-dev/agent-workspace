import { here } from './href'
import { useNavigate } from 'react-router-dom'

import { useShellPage } from '../shell/shellStore'
import { Button, EmptyState } from '../ui'

/** An address that isn't part of the app. Loaded only when it's needed. */
export function NotFoundPage() {
  const navigate = useNavigate()
  useShellPage({ mainClassName: 'aw-main--page' })
  return (
    <div className="aw-page" role="alert">
      <EmptyState
        action={
          <Button onClick={() => navigate(here('/home'))} variant="primary">
            Go to Home
          </Button>
        }
        icon="search"
        title="Page not found"
      >
        That address isn’t part of this workspace. It may have moved, or the link may be old.
      </EmptyState>
    </div>
  )
}
