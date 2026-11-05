import type { ReactNode } from 'react'
import { useLocation, useNavigate, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { useHomeExperience, useHomeWorkspaceId, useProjectExperience } from '../experience/layout'
import { AppShell } from '../shell/AppShell'
import { Button, ButtonLink, EmptyState } from '../ui'
import { moduleName, moduleOfPath } from './slots'

// AWD-75: a link into a module that's off opens this page, not a 404.
// Off means hidden, never deleted, so the page says the thing is still there.

/** Renders `children`, or the turned-off page when the page's module is off for the person. */
export function ModuleGate({ children }: { children: ReactNode }) {
  const { pathname } = useLocation()
  const { projectId } = useParams()
  const project = useProjectExperience(projectId)
  const home = useHomeExperience()
  const module = moduleOfPath(pathname)
  const experience = projectId ? project : home
  if (module && !experience.moduleOn(module)) return <TurnedOffPage module={module} />
  return <>{children}</>
}

export function TurnedOffPage({ module }: { module: string }) {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  const workspaceId = useHomeWorkspaceId()
  const name = moduleName(module)
  const manages = session.organizations.some(
    (organization) => organization.id === workspaceId && (organization.role === 'owner' || organization.role === 'admin'),
  )
  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page" role="status">
        <EmptyState
          action={
            <>
              <Button onClick={() => navigate('/home')} variant="primary">
                Go to Home
              </Button>
              {manages ? <ButtonLink to="/settings/workspace/modules">Turn modules on or off</ButtonLink> : null}
            </>
          }
          icon="info"
          title={`${name} is turned off`}
        >
          {manages
            ? `This workspace has ${name} switched off, so what it holds is hidden. Nothing was deleted: turn it back on in Settings and everything comes back.`
            : `This workspace has ${name} switched off, so what it holds is hidden. Nothing was deleted; a workspace owner or admin can turn it back on.`}
        </EmptyState>
      </div>
    </AppShell>
  )
}
