import { useMemo, useState } from 'react'
import { Link, useLocation, useSearchParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { useProjects } from '../projects/hooks'
import { AppShell } from '../shell/AppShell'
import { Button, EmptyState, ErrorState, Icon, PageHeader, SegmentedControl, SettingsSkeleton } from '../ui'
import { QuickMessageForm, QuickTaskForm } from './QuickAddForms'
import { readSharedContent, type SharedContent } from './sharedContent'
import './quickAdd.css'

/**
 * Quick add (the home-screen "New task" shortcut) and Share to Workspace
 * (what another app shares, through the Web Share Target): one short form,
 * with the button where a thumb reaches it on a phone.
 */
export function QuickAddPage() {
  const session = useAuthenticatedSession()
  const location = useLocation()
  const [params] = useSearchParams()
  const sharing = location.pathname === '/share'
  const shared = useMemo(() => (sharing ? readSharedContent(params) : null), [params, sharing])
  const [mode, setMode] = useState<'task' | 'message'>('task')
  const projects = useProjects()
  const active = useMemo(() => (projects.data?.projects ?? []).filter((project) => !project.archivedAt), [projects.data])

  let body
  if (projects.isPending) {
    body = <SettingsSkeleton />
  } else if (projects.isError) {
    body = (
      <ErrorState
        action={
          <Button icon="retry" onClick={() => void projects.refetch()}>
            Try again
          </Button>
        }
        title="Projects didn’t load"
      >
        {projects.error.message}
      </ErrorState>
    )
  } else if (active.length === 0 && mode === 'task') {
    body = (
      <EmptyState
        action={
          <Link className="aw-btn aw-btn--primary aw-btn--md" to="/projects/new">
            <span className="aw-btn__label">Create a project</span>
          </Link>
        }
        icon="work"
        title="No project to add it to"
      >
        Tasks live in a project. Create one first.
      </EmptyState>
    )
  } else if (mode === 'task') {
    body = <QuickTaskForm projects={active} shared={shared} />
  } else {
    body = <QuickMessageForm projects={active} shared={shared} viewerId={session.user.actorId} />
  }

  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-page--form aw-quick">
        <PageHeader icon={sharing ? 'link' : 'plus'} title={sharing ? 'Share to Workspace' : 'New task'}>
          {sharing ? 'Make it a task, or post it in a conversation.' : 'Add a task to a project. It stays here until it’s created, even offline.'}
        </PageHeader>
        {shared ? <SharedPreview shared={shared} /> : null}
        {sharing ? (
          <SegmentedControl
            label="Share as"
            onChange={setMode}
            options={[
              { value: 'task', label: 'New task', icon: 'work' },
              { value: 'message', label: 'Message', icon: 'message' },
            ]}
            value={mode}
          />
        ) : null}
        {body}
      </div>
    </AppShell>
  )
}

function SharedPreview({ shared }: { shared: SharedContent }) {
  return (
    <section aria-label="What was shared" className="aw-quick__shared">
      {shared.title ? <p className="aw-quick__shared-title">{shared.title}</p> : null}
      {shared.text ? <p className="aw-quick__shared-text">{shared.text}</p> : null}
      {shared.url ? (
        <p className="aw-quick__shared-url">
          <Icon name="link" size={14} />
          <span>{shared.url}</span>
        </p>
      ) : null}
    </section>
  )
}
