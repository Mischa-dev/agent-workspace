import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, Navigate, useLocation, useNavigate, useParams, useSearchParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { readLocal, writeLocal } from '../app/localDrafts'
import { toastError } from '../app/mutations'
import { useAuthenticatedSession } from '../auth/session'
import { ProjectPageError, ProjectPageLoading } from '../projects/ProjectPageStates'
import { useProject } from '../projects/hooks'
import { RememberPlace } from '../projects/RememberPlace'
import type { Project, ProjectLocation } from '../projects/types'
import { useHomeWorkspaceId, useProjectExperience } from '../experience/layout'
import { AppShell } from '../shell/AppShell'
import { MemoryTabs } from '../shell/ProjectTabs'
import { usePageTitle } from '../shell/documentTitleModel'
import {
  Banner,
  Button,
  ButtonLink,
  Checkbox,
  ConfirmDialog,
  Dialog,
  DocumentSkeleton,
  EmptyState,
  ErrorState,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  Section,
  StatusChip,
  TextField,
  describeError,
  formatDateTime,
  formatFullDate,
  formatRelative,
  plainPreview,
  showToast,
} from '../ui'
import { Markdown, MarkdownEditor } from '../ui/markdown'
import type { Embed } from '../ui/markdown/parse'
import { thingLink } from '../modules/references/thingLinks'
import type { Piece, Thing, ThingDecl } from '../modules/contract'
import { PieceView } from '../modules/pieces/Pieces'
import pagesManifest from '../../../../modules/pages.json'
import {
  createPage,
  getPage,
  getPageVersion,
  listDecisions,
  listPages,
  memoryKeys,
  recordDecision,
  updatePage,
  type Decision,
  type Page,
  type PageRevision,
  type PageSummary,
  workspacePagesSpace,
} from './api'
import { discussionKey, getDiscussion, type CommentAnchor } from './discussion/api'
import { PageEmbed } from './embeds/PageEmbed'
import { PageDownloadMenu, PageImportButton } from './PageTransfer'
import type { LiveDiscussion } from './live/LivePage'
import './memory.css'

/** A collection's view, or whatever else a page embeds, live where the page is read. */
const renderEmbed = (embed: Embed) => <PageEmbed href={embed.href} label={embed.label} />

// A page's boards load in the whiteboards' own small chunk.
const LinkedWhiteboards = lazy(() =>
  import('../whiteboards/LinkedWhiteboards').then((module) => ({ default: module.LinkedWhiteboards })),
)
const ReferencedIn = lazy(() => import('../modules/references/ReferencedIn'))
// The live editor (AWD-53) loads in its own chunks when a page opens for writing.
const LivePage = lazy(() => import('./live/LivePage'))
// Comments and suggested changes (AWD-53) load in their own chunk.
const PageDiscussion = lazy(() => import('./discussion/PageDiscussion'))

/** "5 min ago", with the full date on hover. */
function When({ at }: { at: string }) {
  return (
    <time dateTime={at} title={formatFullDate(at)}>
      {formatRelative(at)}
    </time>
  )
}

/**
 * A page's excerpt as plain words. The server sends the start of the page on
 * one line, so the breaks go back in where a Markdown block starts (a
 * heading, list item or quote) for the markers to drop out, and a link the
 * cut ended inside keeps its words.
 */
function excerptText(excerpt: string) {
  const whole = excerpt.replace(/\[([^\]]*)\]\([^)]*$/, '$1').replace(/\[([^\]]*)$/, '$1')
  const blocks = whole.split(/\s+(?=(?:#{1,6}|[-*+]|\d+[.)]|>)\s)/).map((block) => plainPreview(block))
  return plainPreview(blocks.filter(Boolean).join(' · '), 240)
}

/** Loads the project and renders its frame, or the loading and error states, remembering `place` as where the person is. */
function ProjectFrame({ children, place }: { children: (project: Project) => ReactNode; place?: ProjectLocation }) {
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
        <ProjectPageError error={project.error} onRetry={() => void project.refetch()} />
      </AppShell>
    )
  }
  return (
    <AppShell mainClassName="aw-main--page" project={project.data} user={session.user}>
      <div className="aw-page aw-page--reading">
        {place ? <RememberPlace location={place} project={project.data} /> : null}
        {children(project.data)}
      </div>
    </AppShell>
  )
}

const pagesHref = (projectId: string) => `/projects/${encodeURIComponent(projectId)}/pages`

/** The workspace's own pages, outside any project: they live in its hidden space. */
function WorkspacePagesPage() {
  const session = useAuthenticatedSession()
  const organizationId = useHomeWorkspaceId()
  const space = useQuery({
    queryKey: ['organization', organizationId, 'pages-space'],
    queryFn: () => workspacePagesSpace(organizationId ?? ''),
    enabled: Boolean(organizationId),
    staleTime: Infinity,
  })
  if (space.data) return <Navigate replace to={pagesHref(space.data.projectId)} />
  return (
    <AppShell user={session.user}>
      {space.isError ? (
        <ErrorState error={space.error} onRetry={() => void space.refetch()} title="Pages didn’t load" />
      ) : (
        <ListSkeleton label="Loading pages" rows={3} />
      )}
    </AppShell>
  )
}

export function PagesPage() {
  // `/pages`: the workspace's own pages, not a project's.
  if (!useParams().projectId) return <WorkspacePagesPage />
  return <ProjectFrame place={{ kind: 'pages' }}>{(project) => <PagesList projectId={project.id} space={project.workspaceSpace} />}</ProjectFrame>
}

/** A page as the pieces see it: its tags, when and by whom it was last edited, and how it starts. */
const PAGE_THING = pagesManifest.things.find((thing) => thing.kind === 'page') as ThingDecl
const PAGE_LIST: Piece = { type: 'list', thing: 'page', fields: ['tags', 'updatedAt', 'owner', 'excerpt'] }
const DECISION_THING = pagesManifest.things.find((thing) => thing.kind === 'decision') as ThingDecl
const DECISION_LIST: Piece = { type: 'list', thing: 'decision', fields: ['status', 'decidedAt', 'decidedBy', 'source', 'detail'] }

function pageThing(page: PageSummary, base: string): Thing {
  const tags = [page.isGuide && 'Guide', page.skillDescription && 'Skill', page.stale && 'Not updated in months'].filter(
    (tag): tag is string => Boolean(tag),
  )
  return {
    id: page.id,
    kind: 'page',
    module: 'pages',
    title: page.title,
    href: `${base}/${encodeURIComponent(page.id)}`,
    owner: page.updatedBy ? { id: page.updatedBy, name: page.updatedBy } : undefined,
    fields: { tags, updatedAt: page.updatedAt, excerpt: excerptText(page.excerpt) },
  }
}

function PagesList({ projectId, space }: { projectId: string; space?: boolean }) {
  const pages = useQuery({ queryKey: memoryKeys.pages(projectId), queryFn: () => listPages(projectId) })
  const [creating, setCreating] = useState<{ title: string } | null>(null)
  const base = pagesHref(projectId)
  const hasGuide = Boolean(pages.data?.pages.some((page) => page.isGuide))
  const canEdit = Boolean(pages.data?.canEdit)
  const transfers = useProjectExperience(projectId).shows('page.import_export')
  return (
    <>
      <PageHeader
        actions={
          canEdit ? (
            <>
              {transfers ? <PageImportButton pagesHref={base} projectId={projectId} /> : null}
              <Button icon="plus" onClick={() => setCreating({ title: '' })} variant="primary">
                New page
              </Button>
            </>
          ) : null
        }
        icon="list"
        title="Pages"
      />
      <MemoryTabs current="pages" projectId={projectId} />
      {pages.isPending ? <ListSkeleton label="Loading pages" rows={3} /> : null}
      {pages.isError ? <ErrorState error={pages.error} onRetry={() => void pages.refetch()} title="Pages didn’t load" /> : null}
      {pages.data && pages.data.pages.length === 0 ? (
        <EmptyState
          action={
            canEdit ? (
              <Button icon="pencil" onClick={() => setCreating({ title: space ? '' : 'Project guide' })} variant="primary">
                {space ? 'New page' : 'Write the guide'}
              </Button>
            ) : null
          }
          icon="list"
          title="No pages yet"
        >
          {space
            ? 'Pages here belong to the whole workspace: everyone in it, people and agents, can read them.'
            : 'Start with a guide: what this project is and how people and agents should work in it. Agents read it first.'}
        </EmptyState>
      ) : null}
      {pages.data && pages.data.pages.length > 0 ? (
        // AWD-98: drawn by the list piece every module uses.
        <PieceView decl={PAGE_THING} label="Pages" piece={PAGE_LIST} things={pages.data.pages.map((page) => pageThing(page, base))} />
      ) : null}
      {creating ? <NewPageDialog hasGuide={hasGuide || Boolean(space)} initialTitle={creating.title} onClose={() => setCreating(null)} projectId={projectId} space={space} /> : null}
    </>
  )
}

/** Names a new page and opens it in the editor; the first page is the guide unless you say otherwise. */
function NewPageDialog({
  hasGuide,
  initialTitle,
  onClose,
  projectId,
  space,
}: {
  hasGuide: boolean
  initialTitle: string
  onClose: () => void
  projectId: string
  space?: boolean
}) {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [title, setTitle] = useState(initialTitle)
  const [guide, setGuide] = useState(!hasGuide)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!title.trim() || busy) return
    setBusy(true)
    setError(null)
    void createPage(projectId, { title: title.trim(), body: '', isGuide: guide })
      .then(async (page) => {
        await queryClient.invalidateQueries({ queryKey: memoryKeys.pages(projectId) })
        navigate(`${pagesHref(projectId)}/${encodeURIComponent(page.id)}?edit=1`)
      })
      .catch((reason: unknown) => {
        setError(describeError(reason).message)
        setBusy(false)
      })
  }
  return (
    <Dialog
      description="Name it now; the page opens in the editor."
      dirty={title.trim() !== initialTitle.trim()}
      footer={
        <>
          <Button onClick={onClose} variant="secondary">
            Cancel
          </Button>
          <Button busy={busy} disabled={!title.trim()} type="submit" variant="primary">
            Create page
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title="New page"
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      <TextField
        label="Title"
        maxLength={200}
        onChange={(event) => setTitle(event.target.value)}
        placeholder={hasGuide ? 'Theming, How we deploy…' : 'Project guide'}
        value={title}
      />
      {space ? null : (
        <Checkbox
          checked={guide}
          description={hasGuide ? 'It takes over from the current guide.' : 'The guide is what people and agents read first in this project.'}
          label="Make it the project guide"
          onChange={setGuide}
        />
      )}
    </Dialog>
  )
}

export function PageDetailPage() {
  const { pageId = '' } = useParams()
  return <ProjectFrame>{(project) => <PageDetail key={pageId} pageId={pageId} project={project} />}</ProjectFrame>
}

type Viewing = { version: number; title: string; body: string }

function PageDetail({ pageId, project }: { pageId: string; project: Project }) {
  const projectId = project.id
  const queryClient = useQueryClient()
  const page = useQuery({ queryKey: memoryKeys.page(projectId, pageId), queryFn: () => getPage(pageId) })
  usePageTitle(page.data?.title ?? null)
  const pages = useQuery({ queryKey: memoryKeys.pages(projectId), queryFn: () => listPages(projectId) })
  // The editor is part of the address, so a reload keeps it open (with the draft) and saving closes it.
  const [params, setParams] = useSearchParams()
  const editing = params.get('edit') === '1'
  const setEditing = (open: boolean) => {
    const next = new URLSearchParams(params)
    if (open) next.set('edit', '1')
    else next.delete('edit')
    setParams(next, { replace: true })
  }
  const [viewing, setViewing] = useState<Viewing | null>(null)
  const [loadingVersion, setLoadingVersion] = useState<number | null>(null)
  const [restoring, setRestoring] = useState(false)
  const viewer = useRef<HTMLDivElement>(null)
  const canEdit = Boolean(pages.data?.canEdit)
  const experience = useProjectExperience(projectId)
  const showsDiscussion = experience.shows('page.comments')
  const discussion = useQuery({
    queryKey: discussionKey(projectId, pageId),
    queryFn: () => getDiscussion(pageId),
    enabled: showsDiscussion,
  })
  const [draft, setDraft] = useState<CommentAnchor | null>(null)
  const [activeThread, setActiveThread] = useState<string | null>(null)
  const startComment = useCallback((anchor: CommentAnchor) => {
    setDraft(anchor)
    window.requestAnimationFrame(() => document.getElementById('page-discussion')?.scrollIntoView({ behavior: 'smooth', block: 'start' }))
  }, [])
  const liveDiscussion = useMemo<LiveDiscussion | null>(() => {
    if (!showsDiscussion || !discussion.data?.canComment) return null
    return {
      threads: discussion.data.threads.flatMap((thread) =>
        !thread.resolved && thread.blockId && thread.quote ? [{ id: thread.id, blockId: thread.blockId, quote: thread.quote }] : [],
      ),
      active: activeThread,
      suggested: discussion.data.suggestions.flatMap((suggestion) =>
        suggestion.status === 'pending' ? suggestion.edits.flatMap((edit) => (edit.blockId && (edit.op === 'replace' || edit.op === 'delete') ? [edit.blockId] : [])) : [],
      ),
      onComment: startComment,
      onOpenThread: setActiveThread,
    }
  }, [showsDiscussion, discussion.data, activeThread, startComment])

  // Opening an old version shows it where the page is, so bring it into view.
  useEffect(() => {
    if (!viewing || !viewer.current) return
    viewer.current.scrollIntoView({ behavior: 'smooth', block: 'start' })
    viewer.current.focus({ preventScroll: true })
  }, [viewing])

  const refresh = async (saved: Page) => {
    queryClient.setQueryData(memoryKeys.page(projectId, pageId), saved)
    await queryClient.invalidateQueries({ queryKey: memoryKeys.pages(projectId) })
  }
  if (page.isPending) return <DocumentSkeleton label="Loading page" />
  if (page.isError && isApiError(page.error) && page.error.status === 404) {
    return (
      <EmptyState action={<ButtonLink to={pagesHref(projectId)}>All pages</ButtonLink>} icon="list" title="This page isn’t here">
        It may have been archived, or it’s in a project you’re not in.
      </EmptyState>
    )
  }
  if (page.isError) return <ErrorState error={page.error} onRetry={() => void page.refetch()} title="This page didn’t load" />
  const current = page.data
  const open = (version: number) => {
    setLoadingVersion(version)
    void getPageVersion(pageId, version)
      .then((loaded) => setViewing({ version: loaded.version, title: loaded.title, body: loaded.body }))
      .catch((reason: unknown) => toastError('That version didn’t load', reason))
      .finally(() => setLoadingVersion(null))
  }
  const restore = (version: number) => {
    setRestoring(true)
    void updatePage(pageId, { restoreVersion: version, expectedVersion: current.version })
      .then(async (saved) => {
        setViewing(null)
        await refresh(saved)
        showToast({ title: `Version ${version} restored`, description: `It’s saved as version ${saved.version}.` })
      })
      .catch((reason: unknown) => toastError('That version wasn’t restored', reason))
      .finally(() => setRestoring(false))
  }
  // Someone who may edit writes in the page itself, live with everyone who
  // has it open; Edit details still opens the form for its title, settings
  // and Markdown.
  const live = canEdit && !current.archived && experience.shows('page.live_editing')
  const read = (
    <Markdown
      className="aw-doc"
      empty={live ? 'Nothing here yet.' : canEdit ? 'Nothing here yet. Choose Edit to write it.' : 'Nothing here yet.'}
      renderEmbed={renderEmbed}
      renderLink={thingLink}
      source={current.body}
    />
  )
  return (
    <>
      <RememberPlace location={{ kind: 'page', pageId }} project={project} />
      <PageHeader
        actions={
          editing ? null : (
            <>
              {experience.shows('page.import_export') ? <PageDownloadMenu pageId={pageId} /> : null}
              {canEdit ? (
                live ? (
                  <Button icon="settings" onClick={() => setEditing(true)}>
                    Edit details
                  </Button>
                ) : (
                  <Button icon="pencil" onClick={() => setEditing(true)} variant="primary">
                    Edit
                  </Button>
                )
              ) : null}
            </>
          )
        }
        icon="list"
        title={current.title}
      >
        <Link to={pagesHref(projectId)}>All pages</Link>
        {' · '}Version {current.version} · edited <When at={current.updatedAt} />
        {current.updatedBy ? ` by ${current.updatedBy}` : ''}
      </PageHeader>
      {current.stale ? (
        <Banner tone="warning" title="Nobody has touched this in months.">
          Check it’s still right before relying on it.
        </Banner>
      ) : null}
      {current.archived ? <Banner tone="offline">This page is archived.</Banner> : null}
      {current.skillDescription && !editing ? (
        <Banner tone="agent" title="Skill">
          Agents read this page when it fits their work: {current.skillDescription}
        </Banner>
      ) : null}
      {editing && canEdit ? (
        <PageEditor
          onCancel={() => setEditing(false)}
          onSaved={async (saved) => {
            await refresh(saved)
            setEditing(false)
            showToast({ title: 'Page saved' })
          }}
          page={current}
          projectId={projectId}
          space={project.workspaceSpace}
        />
      ) : viewing ? (
        <div className="aw-memory__viewing" ref={viewer} tabIndex={-1}>
          <Section
            actions={
              <>
                {canEdit ? (
                  <Button busy={restoring} onClick={() => restore(viewing.version)} size="sm" variant="primary">
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
            <Markdown className="aw-doc" empty="This version was empty." renderEmbed={renderEmbed} renderLink={thingLink} source={viewing.body} />
          </Section>
        </div>
      ) : live ? (
        <Suspense fallback={<div className="aw-memory__live-wait">{read}</div>}>
          <LivePage
            collections={experience.shows('area.collections')}
            discussion={liveDiscussion}
            fallback={read}
            pageId={pageId}
            projectId={projectId}
            sheets={experience.shows('area.sheets')}
            title={current.title}
          />
        </Suspense>
      ) : (
        read
      )}
      {showsDiscussion && discussion.data && !editing ? (
        <Suspense fallback={null}>
          <PageDiscussion
            active={activeThread}
            discussion={discussion.data}
            draft={draft}
            onActivate={setActiveThread}
            onDraftDone={() => setDraft(null)}
            pageId={pageId}
            projectId={projectId}
          />
        </Suspense>
      ) : null}
      <Suspense fallback={null}>
        <LinkedWhiteboards canEdit={canEdit && !current.archived} projectId={projectId} targetId={current.id} targetKind="page" />
      </Suspense>
      {experience.shows('page.references') ? (
        <Suspense fallback={null}>
          <ReferencedIn id={current.id} kind="page" />
        </Suspense>
      ) : null}
      <Section description="Every save is kept. Open an old version to read it or put it back." title="History">
        <RowList label="Versions">
          {current.revisions.map((revision) => (
            <Row
              actions={
                revision.version === current.version ? (
                  <StatusChip>Current</StatusChip>
                ) : viewing?.version === revision.version ? (
                  <StatusChip>Showing above</StatusChip>
                ) : (
                  <Button busy={loadingVersion === revision.version} disabled={loadingVersion !== null} onClick={() => open(revision.version)} size="sm">
                    View
                  </Button>
                )
              }
              key={revision.version}
              meta={
                <span>
                  <time dateTime={revision.createdAt} title={formatFullDate(revision.createdAt)}>
                    {formatDateTime(revision.createdAt)}
                  </time>
                  {revision.author ? ` · ${revision.author}` : ''}
                  {revision.via ? ` via ${revision.via}` : ''}
                  <RevisionSession projectId={projectId} revision={revision} />
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

/** What the editor keeps in this browser until it's saved: the version it started from and the text. */
type PageDraft = { version: number; title: string; body: string; note: string }

const draftKey = (pageId: string) => `page-draft:${pageId}`

function PageEditor({
  onCancel,
  onSaved,
  page,
  projectId,
  space,
}: {
  onCancel: () => void
  onSaved: (page: Page) => Promise<void>
  page: Page
  projectId: string
  space?: boolean
}) {
  // The version this edit started from, kept even when the page refetches
  // underneath, so a save can't quietly replace someone else's newer one.
  const [restored] = useState(() => {
    const draft = readLocal<PageDraft>(draftKey(page.id))
    return draft && (draft.title !== page.title || draft.body !== page.body || draft.note) ? draft : null
  })
  const [base, setBase] = useState(restored?.version ?? page.version)
  const [original] = useState({ title: page.title, body: page.body })
  const [title, setTitle] = useState(restored?.title ?? page.title)
  const [body, setBody] = useState(restored?.body ?? page.body)
  const [note, setNote] = useState(restored?.note ?? '')
  const [guide, setGuide] = useState(page.isGuide)
  const [skill, setSkill] = useState(page.skillDescription !== null)
  const [skillDescription, setSkillDescription] = useState(page.skillDescription ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [discarding, setDiscarding] = useState(false)
  const queryClient = useQueryClient()
  const dirty = title !== original.title || body !== original.body || Boolean(note.trim())
  const newer = page.version !== base

  // Keep the draft in this browser, and ask before the tab closes on it.
  useEffect(() => {
    writeLocal<PageDraft>(draftKey(page.id), dirty ? { version: base, title, body, note } : null)
  }, [base, body, dirty, note, page.id, title])
  useEffect(() => {
    if (!dirty) return
    const warn = (event: BeforeUnloadEvent) => {
      event.preventDefault()
      event.returnValue = ''
    }
    window.addEventListener('beforeunload', warn)
    return () => window.removeEventListener('beforeunload', warn)
  }, [dirty])

  const discard = () => {
    writeLocal(draftKey(page.id), null)
    onCancel()
  }
  /** Saves over `expected`: the version this edit started from, or theirs once you choose to replace it. */
  const submit = (expected: number) => {
    if (!title.trim() || busy || (skill && !skillDescription.trim())) return
    setBusy(true)
    setError(null)
    setBase(expected)
    void updatePage(page.id, {
      title: title.trim(),
      body,
      isGuide: guide,
      skillDescription: skill ? skillDescription.trim() : '',
      expectedVersion: expected,
      note: note.trim() || undefined,
    })
      .then(async (saved) => {
        writeLocal(draftKey(page.id), null)
        await onSaved(saved)
      })
      .catch((reason: unknown) => {
        if (isApiError(reason) && reason.status === 409) {
          // Fetch their version underneath; the draft stays in the editor.
          void queryClient.invalidateQueries({ queryKey: memoryKeys.page(projectId, page.id) })
          setError('Someone saved a newer version while you were editing. Your text is still here.')
          return
        }
        setError(describeError(reason).message)
      })
      .finally(() => setBusy(false))
  }
  const save = (event: FormEvent) => {
    event.preventDefault()
    if (!newer) submit(base)
  }
  return (
    <form className="aw-page-editor" onSubmit={save}>
      {restored ? (
        <Banner tone="info" title="Your unsaved changes are back.">
          They were kept in this browser.
        </Banner>
      ) : null}
      {newer ? (
        <Banner
          action={
            <Button busy={busy} disabled={!title.trim() || (skill && !skillDescription.trim())} onClick={() => submit(page.version)} size="sm">
              Save mine over it
            </Button>
          }
          tone="warning"
          title={`${page.updatedBy ?? 'Someone'} saved version ${page.version} while you were editing.`}
        >
          Saving yours replaces it. To read theirs first, open the page in another tab.
        </Banner>
      ) : null}
      {error && !newer ? <Banner tone="danger">{error}</Banner> : null}
      <TextField label="Title" maxLength={200} onChange={(event) => setTitle(event.target.value)} value={title} />
      <MarkdownEditor
        hint="Markdown: headings, lists, - [ ] checklists, ``` code, links. Task keys like AWD-12 link to the task."
        label="Page"
        onChange={setBody}
        rows={20}
        value={body}
      />
      {space ? null : (
        <Checkbox
          checked={guide}
          description="The guide is what people and agents read first in this project."
          label="This is the project guide"
          onChange={setGuide}
        />
      )}
      <Checkbox
        checked={skill}
        description="Agents see the list of skills in a project and follow one when its description fits their work."
        label="Agents can use this as a skill"
        onChange={setSkill}
      />
      {skill ? (
        <TextField
          hint="One line, starting with when: “Use when deploying the web app.”"
          label="When agents should use it"
          maxLength={500}
          onChange={(event) => setSkillDescription(event.target.value)}
          value={skillDescription}
        />
      ) : null}
      <TextField label="What changed (optional)" maxLength={500} onChange={(event) => setNote(event.target.value)} value={note} />
      <div className="aw-form__actions">
        <Button onClick={() => (dirty ? setDiscarding(true) : onCancel())} variant="secondary">
          Cancel
        </Button>
        <Button busy={busy} disabled={newer || !title.trim() || (skill && !skillDescription.trim())} type="submit" variant="primary">
          Save
        </Button>
      </div>
      <ConfirmDialog confirmLabel="Discard changes" onClose={() => setDiscarding(false)} onConfirm={discard} open={discarding} title="Discard your changes?">
        What you typed since opening the editor is lost.
      </ConfirmDialog>
    </form>
  )
}

export function DecisionsPage() {
  return <ProjectFrame place={{ kind: 'decisions' }}>{(project) => <DecisionsList projectId={project.id} />}</ProjectFrame>
}

/** Where a decision was made, as the app links to it: "#design", a thread's name, or a DM. */
function sourceLink(decision: Decision) {
  const { sourceConversationId: id, sourceConversationKind: kind, sourceConversationName: name, sourceMessageId: messageId } = decision
  if (!id) return null
  const anchor = messageId ? `#message-${messageId}` : ''
  const project = encodeURIComponent(decision.projectId)
  if (kind === 'channel') return { label: `#${name ?? 'channel'}`, to: `/projects/${project}/channels/${encodeURIComponent(id)}${anchor}` }
  if (kind === 'thread') return { label: name ?? 'a thread', to: `/projects/${project}/threads/${encodeURIComponent(id)}${anchor}` }
  if (kind === 'dm' || kind === 'group') return { label: name ?? 'a direct message', to: `/messages/${encodeURIComponent(id)}${anchor}` }
  return name ? { label: name, to: null } : null
}

/** A decision as the list piece draws it; its row is `#decision-<id>`, which a link to it scrolls to. */
function decisionThing(decision: Decision): Thing {
  const source = sourceLink(decision)
  return {
    id: decision.id,
    kind: 'decision',
    module: 'pages',
    title: decision.title,
    status: decision.status === 'superseded' ? 'Replaced' : undefined,
    projectId: decision.projectId,
    href: `/projects/${encodeURIComponent(decision.projectId)}/decisions#${encodeURIComponent(decision.id)}`,
    fields: {
      decidedAt: decision.createdAt,
      decidedBy: decision.via ? `${decision.decidedBy} via ${decision.via}` : decision.decidedBy,
      source: source ? { label: source.label, href: source.to } : null,
      detail: decision.detail,
    },
  }
}

function DecisionsList({ projectId }: { projectId: string }) {
  const location = useLocation()
  const [showSuperseded, setShowSuperseded] = useState(false)
  const [recording, setRecording] = useState(false)
  const decisions = useQuery({
    queryKey: [...memoryKeys.decisions(projectId), showSuperseded],
    queryFn: () => listDecisions(projectId, showSuperseded),
  })
  const target = location.hash ? decodeURIComponent(location.hash.slice(1)) : ''

  // A link to one decision (from a pin) scrolls to it and marks it for a moment.
  useEffect(() => {
    if (!target || !decisions.data) return
    const row = document.getElementById(`decision-${target}`)
    if (!row) return
    row.scrollIntoView({ behavior: 'smooth', block: 'center' })
    row.classList.add('aw-memory__found')
    const timer = window.setTimeout(() => row.classList.remove('aw-memory__found'), 2400)
    return () => {
      window.clearTimeout(timer)
      row.classList.remove('aw-memory__found')
    }
  }, [decisions.data, target])

  const canRecord = Boolean(decisions.data?.canRecord)
  return (
    <>
      <PageHeader
        actions={
          canRecord ? (
            <Button icon="plus" onClick={() => setRecording(true)} variant="primary">
              New decision
            </Button>
          ) : null
        }
        icon="check"
        title="Decisions"
      />
      <MemoryTabs current="decisions" projectId={projectId} />
      <div className="aw-memory__toolbar">
        <Checkbox checked={showSuperseded} label="Show replaced decisions" onChange={setShowSuperseded} />
      </div>
      {decisions.isPending ? <ListSkeleton label="Loading decisions" rows={3} /> : null}
      {decisions.isError ? <ErrorState error={decisions.error} onRetry={() => void decisions.refetch()} title="Decisions didn’t load" /> : null}
      {decisions.data && decisions.data.decisions.length === 0 ? (
        <EmptyState
          action={
            canRecord ? (
              <Button icon="plus" onClick={() => setRecording(true)}>
                Record a decision
              </Button>
            ) : null
          }
          icon="check"
          title="No decisions yet"
        >
          When a discussion settles something, save the message as a decision so nobody has to reread the thread.
        </EmptyState>
      ) : null}
      {decisions.data && decisions.data.decisions.length > 0 ? (
        <PieceView decl={DECISION_THING} label="Decisions" piece={DECISION_LIST} things={decisions.data.decisions.map(decisionThing)} />
      ) : null}
      {recording ? <RecordDecisionDialog onClose={() => setRecording(false)} projectId={projectId} /> : null}
    </>
  )
}

function RecordDecisionDialog({ onClose, projectId }: { onClose: () => void; projectId: string }) {
  const queryClient = useQueryClient()
  const [title, setTitle] = useState('')
  const [detail, setDetail] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!title.trim() || busy) return
    setBusy(true)
    setError(null)
    void recordDecision(projectId, { title: title.trim(), detail: detail.trim() || undefined })
      .then(async () => {
        await queryClient.invalidateQueries({ queryKey: memoryKeys.decisions(projectId) })
        showToast({ title: 'Decision recorded' })
        onClose()
      })
      .catch((reason: unknown) => {
        setError(describeError(reason).message)
        setBusy(false)
      })
  }
  return (
    <Dialog
      description="Decisions are kept with the project, so people and agents can find what was settled and why."
      dirty={Boolean(title.trim() || detail.trim())}
      footer={
        <>
          <Button onClick={onClose} variant="secondary">
            Cancel
          </Button>
          <Button busy={busy} disabled={!title.trim()} type="submit" variant="primary">
            Record decision
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title="Record a decision"
      width={560}
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      <TextField label="What was decided" maxLength={300} onChange={(event) => setTitle(event.target.value)} value={title} />
      <MarkdownEditor label="Why (optional)" onChange={setDetail} rows={4} value={detail} />
    </Dialog>
  )
}

/** "claude_code" → "Claude Code". */
const harnessName = (kind: string) =>
  kind
    .split(/[_\s-]+/)
    .filter(Boolean)
    .map((word) => word.charAt(0).toLocaleUpperCase() + word.slice(1))
    .join(' ')

/** Which agent session wrote a version (AWD-53): its run, or the thread or channel it works in. */
function RevisionSession({ projectId, revision }: { projectId: string; revision: PageRevision }) {
  if (!revision.sessionHarness) return null
  const name = harnessName(revision.sessionHarness)
  const conversation = revision.sessionConversationId
  const where =
    conversation && revision.sessionConversationKind === 'channel'
      ? { href: `/projects/${encodeURIComponent(projectId)}/channels/${encodeURIComponent(conversation)}`, label: `#${revision.sessionConversationName ?? 'channel'}` }
      : conversation && revision.sessionConversationKind === 'thread'
        ? { href: `/projects/${encodeURIComponent(projectId)}/threads/${encodeURIComponent(conversation)}`, label: revision.sessionConversationName ?? 'a thread' }
        : null
  return (
    <>
      {' · '}
      {revision.sessionRunId ? <Link to={`/runs/${encodeURIComponent(revision.sessionRunId)}`}>{`${name} run`}</Link> : `${name} session`}
      {where ? (
        <>
          {' in '}
          <Link to={where.href}>{where.label}</Link>
        </>
      ) : null}
    </>
  )
}
