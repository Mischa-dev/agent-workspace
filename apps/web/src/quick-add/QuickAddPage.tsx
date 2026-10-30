import { useEffect, useMemo, useState } from 'react'
import { useLocation, useSearchParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { useProjects } from '../projects/hooks'
import { AppShell } from '../shell/AppShell'
import { ButtonLink, EmptyState, ErrorState, Icon, PageHeader, SegmentedControl, SettingsSkeleton } from '../ui'
import { formatBytes } from '../files/api'
import { QuickMessageForm, QuickTaskForm } from './QuickAddForms'
import { takeSharedFiles } from './sharedFiles'
import { readSharedContent, type SharedContent } from './sharedContent'
import './quickAdd.css'
import { FileGlyph, ImageGlyph } from '../files/icons'

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
  const fileCount = sharing ? Number(params.get('files') ?? 0) : 0
  const [files, setFiles] = useState<File[] | null>(fileCount > 0 ? null : [])
  useEffect(() => {
    if (fileCount <= 0) return
    let current = true
    void takeSharedFiles(fileCount).then((taken) => {
      if (current) setFiles(taken)
    })
    return () => {
      current = false
    }
  }, [fileCount])
  const [mode, setMode] = useState<'task' | 'message'>('task')
  const projects = useProjects()
  const active = useMemo(() => (projects.data?.projects ?? []).filter((project) => !project.archivedAt), [projects.data])

  let body
  if (projects.isPending || files === null) {
    body = <SettingsSkeleton />
  } else if (projects.isError) {
    body = <ErrorState error={projects.error} onRetry={() => void projects.refetch()} title="Projects didn’t load" />
  } else if (active.length === 0 && mode === 'task') {
    body = (
      <EmptyState
        action={
          <ButtonLink icon="plus" to="/projects/new" variant="primary">
            New project
          </ButtonLink>
        }
        icon="work"
        title="No project to add it to"
      >
        Tasks live in a project. Create one first.
      </EmptyState>
    )
  } else if (mode === 'task') {
    body = <QuickTaskForm files={files} projects={active} shared={shared} />
  } else {
    body = <QuickMessageForm files={files} projects={active} shared={shared} viewerId={session.user.actorId} />
  }

  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-page--reading aw-quick">
        <PageHeader icon={sharing ? 'link' : 'plus'} title={sharing ? 'Share' : 'New task'} />
        {shared ? <SharedPreview shared={shared} /> : null}
        {files?.length ? (
          <ul aria-label="Shared files" className="aw-quick__files">
            {files.map((file, index) => (
              <li key={`${file.name}-${index}`}>
                <Icon name={file.type.startsWith('image/') ? ImageGlyph : FileGlyph} size={14} />
                <span className="aw-quick__file-name">{file.name}</span>
                <span className="aw-quick__file-size">{formatBytes(file.size)}</span>
              </li>
            ))}
          </ul>
        ) : null}
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
