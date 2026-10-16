import { useState, type FormEvent, type ReactNode } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { formatFullDate } from '../conversations/timeFormat'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { AppShell } from '../shell/AppShell'
import {
  Banner,
  Button,
  Checkbox,
  EmptyState,
  Markdown,
  MarkdownEditor,
  MessageSkeleton,
  PageHeader,
  Row,
  RowList,
  Section,
  StatusChip,
  TextField,
} from '../ui'
import {
  createPage,
  getPage,
  getPageVersion,
  listDecisions,
  listPages,
  memoryKeys,
  recordDecision,
  updatePage,
  type Page,
} from './api'

function errorText(reason: unknown, fallback: string) {
  return isApiError(reason) || reason instanceof Error ? reason.message : fallback
}

/** Loads the project and renders its frame, or the loading and error states. */
function ProjectFrame({ children }: { children: (projectId: string) => ReactNode }) {
  const session = useAuthenticatedSession()
  const { projectId = '' } = useParams()
  const project = useProject(projectId)
  if (project.isPending) {
    return (
      <AppShell user={session.user}>
        <ProjectPageLoading />
      </AppShell>
    )
  }
  if (project.isError) {
    return (
      <AppShell user={session.user}>
        <ProjectPageError onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }
  return (
    <AppShell mainClassName="aw-main--page" project={project.data} user={session.user}>
      <div className="aw-page">{children(project.data.id)}</div>
    </AppShell>
  )
}

export function PagesPage() {
  return <ProjectFrame>{(projectId) => <PagesList projectId={projectId} />}</ProjectFrame>
}

function PagesList({ projectId }: { projectId: string }) {
  const navigate = useNavigate()
  const pages = useQuery({ queryKey: memoryKeys.pages(projectId), queryFn: () => listPages(projectId) })
  const [title, setTitle] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const base = `/projects/${encodeURIComponent(projectId)}/pages`
  const create = (event: FormEvent) => {
    event.preventDefault()
    if (!title.trim() || busy) return
    setBusy(true)
    setError(null)
    const isGuide = !pages.data?.pages.some((page) => page.isGuide)
    void createPage(projectId, { title: title.trim(), body: '', isGuide })
      .then((page) => navigate(`${base}/${encodeURIComponent(page.id)}?edit=1`))
      .catch((reason: unknown) => setError(errorText(reason, 'The page wasn’t created.')))
      .finally(() => setBusy(false))
  }
  return (
    <>
      <PageHeader icon="list" title="Pages">
        What’s true for this project now: how it works, conventions, plans. People and agents both read and edit these, and every
        version is kept.
      </PageHeader>
      {pages.isPending ? <MessageSkeleton label="Loading pages" rows={2} /> : null}
      {pages.isError ? <Banner tone="danger">Pages didn’t load.</Banner> : null}
      {pages.data && pages.data.pages.length === 0 ? (
        <EmptyState icon="list" title="No pages yet">
          Start with a guide: what this project is and how people and agents should work in it. Agents read it first.
        </EmptyState>
      ) : null}
      {pages.data && pages.data.pages.length > 0 ? (
        <RowList label="Pages">
          {pages.data.pages.map((page) => (
            <Row
              key={page.id}
              meta={
                <>
                  {page.isGuide ? <StatusChip tone="agent">Guide</StatusChip> : null}
                  {page.stale ? <StatusChip tone="warning">Not updated in months</StatusChip> : null}
                  <span>
                    Edited {formatFullDate(page.updatedAt)}
                    {page.updatedBy ? ` by ${page.updatedBy}` : ''}
                  </span>
                </>
              }
              title={<Link to={`${base}/${encodeURIComponent(page.id)}`}>{page.title}</Link>}
            >
              {page.excerpt ? <p className="aw-row__text">{page.excerpt}</p> : null}
            </Row>
          ))}
        </RowList>
      ) : null}
      {pages.data?.canEdit ? (
        <Section title="New page">
          <form className="aw-inline-form" onSubmit={create}>
            <TextField
              id="new-page-title"
              label="Title"
              maxLength={200}
              onChange={(event) => setTitle(event.target.value)}
              placeholder={pages.data.pages.some((page) => page.isGuide) ? 'Theming, How we deploy…' : 'Project guide'}
              value={title}
            />
            <Button busy={busy} disabled={!title.trim()} icon="plus" type="submit" variant="primary">
              Create page
            </Button>
          </form>
          {error ? <Banner tone="danger">{error}</Banner> : null}
        </Section>
      ) : null}
    </>
  )
}

export function PageDetailPage() {
  const { pageId = '' } = useParams()
  return <ProjectFrame>{(projectId) => <PageDetail key={pageId} pageId={pageId} projectId={projectId} />}</ProjectFrame>
}

function PageDetail({ pageId, projectId }: { pageId: string; projectId: string }) {
  const queryClient = useQueryClient()
  const page = useQuery({ queryKey: memoryKeys.page(pageId), queryFn: () => getPage(pageId) })
  const pages = useQuery({ queryKey: memoryKeys.pages(projectId), queryFn: () => listPages(projectId) })
  const [editing, setEditing] = useState(() => new URLSearchParams(window.location.search).get('edit') === '1')
  const [viewing, setViewing] = useState<{ version: number; title: string; body: string } | null>(null)
  const [error, setError] = useState<string | null>(null)
  const canEdit = Boolean(pages.data?.canEdit)
  const refresh = async (saved: Page) => {
    queryClient.setQueryData(memoryKeys.page(pageId), saved)
    await queryClient.invalidateQueries({ queryKey: memoryKeys.pages(projectId) })
  }
  if (page.isPending) return <MessageSkeleton label="Loading page" rows={3} />
  if (page.isError || !page.data) return <Banner tone="danger">This page didn’t load.</Banner>
  const current = page.data
  const restore = (version: number) => {
    setError(null)
    void updatePage(pageId, { restoreVersion: version, expectedVersion: current.version })
      .then(async (saved) => {
        setViewing(null)
        await refresh(saved)
      })
      .catch((reason: unknown) => setError(errorText(reason, 'That version wasn’t restored.')))
  }
  return (
    <>
      <PageHeader
        actions={
          canEdit && !editing ? (
            <Button icon="pencil" onClick={() => setEditing(true)} variant="primary">
              Edit
            </Button>
          ) : null
        }
        icon="list"
        title={current.title}
      >
        <Link to={`/projects/${encodeURIComponent(projectId)}/pages`}>All pages</Link>
        {' · '}Version {current.version}, edited {formatFullDate(current.updatedAt)}
        {current.updatedBy ? ` by ${current.updatedBy}` : ''}
      </PageHeader>
      {current.stale ? (
        <Banner tone="warning" title="Nobody has touched this in months.">
          Check it’s still right before relying on it.
        </Banner>
      ) : null}
      {current.archived ? <Banner tone="offline">This page is archived.</Banner> : null}
      {error ? <Banner tone="danger">{error}</Banner> : null}
      {editing ? (
        <PageEditor onCancel={() => setEditing(false)} onSaved={async (saved) => { await refresh(saved); setEditing(false) }} page={current} />
      ) : viewing ? (
        <Section
          actions={
            <>
              {canEdit ? (
                <Button onClick={() => restore(viewing.version)} size="sm" variant="primary">
                  Restore this version
                </Button>
              ) : null}
              <Button onClick={() => setViewing(null)} size="sm">
                Back to current
              </Button>
            </>
          }
          title={`Version ${viewing.version}: ${viewing.title}`}
        >
          <Markdown className="aw-doc" empty="This version was empty." source={viewing.body} />
        </Section>
      ) : (
        <Markdown
          className="aw-doc"
          empty={canEdit ? 'Nothing here yet. Choose Edit to write it.' : 'Nothing here yet.'}
          source={current.body}
        />
      )}
      <Section description="Every save is kept. Open an old version to read it or put it back." title="History">
        <RowList label="Versions">
          {current.revisions.map((revision) => (
            <Row
              actions={
                revision.version === current.version ? (
                  <StatusChip>Current</StatusChip>
                ) : (
                  <Button
                    onClick={() =>
                      void getPageVersion(pageId, revision.version)
                        .then((version) => setViewing({ version: version.version, title: version.title, body: version.body }))
                        .catch(() => setError('That version didn’t load.'))
                    }
                    size="sm"
                  >
                    View
                  </Button>
                )
              }
              key={revision.version}
              meta={
                <span>
                  {formatFullDate(revision.createdAt)}
                  {revision.author ? ` · ${revision.author}` : ''}
                  {revision.via ? ` via ${revision.via}` : ''}
                </span>
              }
              title={`Version ${revision.version}${revision.note ? `: ${revision.note}` : ''}`}
            />
          ))}
        </RowList>
      </Section>
    </>
  )
}

function PageEditor({
  onCancel,
  onSaved,
  page,
}: {
  onCancel: () => void
  onSaved: (page: Page) => Promise<void>
  page: Page
}) {
  const [title, setTitle] = useState(page.title)
  const [body, setBody] = useState(page.body)
  const [guide, setGuide] = useState(page.isGuide)
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const save = (event: FormEvent) => {
    event.preventDefault()
    if (!title.trim() || busy) return
    setBusy(true)
    setError(null)
    void updatePage(page.id, {
      title: title.trim(),
      body,
      isGuide: guide,
      expectedVersion: page.version,
      note: note.trim() || undefined,
    })
      .then(onSaved)
      .catch((reason: unknown) =>
        setError(
          isApiError(reason) && reason.status === 409
            ? 'Someone changed this page while you were editing. Copy your text, reload, and try again.'
            : errorText(reason, 'The page wasn’t saved.'),
        ),
      )
      .finally(() => setBusy(false))
  }
  return (
    <form className="aw-page-editor" onSubmit={save}>
      {error ? <Banner tone="danger">{error}</Banner> : null}
      <TextField label="Title" maxLength={200} onChange={(event) => setTitle(event.target.value)} value={title} />
      <MarkdownEditor
        hint="Markdown: headings, lists, - [ ] checklists, ``` code, links. Task keys like AWD-12 link to the task."
        label="Page"
        onChange={setBody}
        rows={20}
        value={body}
      />
      <Checkbox
        checked={guide}
        description="The guide is what people and agents read first in this project."
        label="This is the project guide"
        onChange={setGuide}
      />
      <TextField label="What changed (optional)" maxLength={500} onChange={(event) => setNote(event.target.value)} value={note} />
      <div className="aw-form__actions">
        <Button onClick={onCancel}>Cancel</Button>
        <Button busy={busy} disabled={!title.trim()} type="submit" variant="primary">
          Save
        </Button>
      </div>
    </form>
  )
}

export function DecisionsPage() {
  return <ProjectFrame>{(projectId) => <DecisionsList projectId={projectId} />}</ProjectFrame>
}

function DecisionsList({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient()
  const [showSuperseded, setShowSuperseded] = useState(false)
  const decisions = useQuery({
    queryKey: [...memoryKeys.decisions(projectId), showSuperseded],
    queryFn: () => listDecisions(projectId, showSuperseded),
  })
  const [title, setTitle] = useState('')
  const [detail, setDetail] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const record = (event: FormEvent) => {
    event.preventDefault()
    if (!title.trim() || busy) return
    setBusy(true)
    setError(null)
    void recordDecision(projectId, { title: title.trim(), detail: detail.trim() || undefined })
      .then(async () => {
        setTitle('')
        setDetail('')
        await queryClient.invalidateQueries({ queryKey: memoryKeys.decisions(projectId) })
      })
      .catch((reason: unknown) => setError(errorText(reason, 'The decision wasn’t saved.')))
      .finally(() => setBusy(false))
  }
  return (
    <>
      <PageHeader icon="check" title="Decisions">
        What was settled and why. Save one from any message, or record it here. A decision from a private conversation is only
        visible to the people in it.
      </PageHeader>
      <Checkbox checked={showSuperseded} label="Show replaced decisions" onChange={setShowSuperseded} />
      {decisions.isPending ? <MessageSkeleton label="Loading decisions" rows={2} /> : null}
      {decisions.isError ? <Banner tone="danger">Decisions didn’t load.</Banner> : null}
      {decisions.data && decisions.data.decisions.length === 0 ? (
        <EmptyState icon="check" title="No decisions yet">
          When a discussion settles something, save the message as a decision so nobody has to reread the thread.
        </EmptyState>
      ) : null}
      {decisions.data && decisions.data.decisions.length > 0 ? (
        <RowList label="Decisions">
          {decisions.data.decisions.map((decision) => (
            <Row
              key={decision.id}
              meta={
                <>
                  {decision.status === 'superseded' ? <StatusChip>Replaced</StatusChip> : null}
                  <span>
                    {decision.decidedBy}
                    {decision.via ? ` via ${decision.via}` : ''} · {formatFullDate(decision.createdAt)}
                    {decision.sourceConversationName ? ` · in ${decision.sourceConversationName}` : ''}
                  </span>
                </>
              }
              title={<span id={decision.id}>{decision.title}</span>}
            >
              {decision.detail ? <Markdown className="aw-row__text" source={decision.detail} /> : null}
            </Row>
          ))}
        </RowList>
      ) : null}
      {decisions.data?.canRecord ? (
        <Section title="Record a decision">
          <form className="aw-page-editor" onSubmit={record}>
            {error ? <Banner tone="danger">{error}</Banner> : null}
            <TextField label="What was decided" maxLength={300} onChange={(event) => setTitle(event.target.value)} value={title} />
            <MarkdownEditor label="Why (optional)" onChange={setDetail} rows={3} value={detail} />
            <div className="aw-form__actions">
              <Button busy={busy} disabled={!title.trim()} type="submit" variant="primary">
                Record decision
              </Button>
            </div>
          </form>
        </Section>
      ) : null}
    </>
  )
}
