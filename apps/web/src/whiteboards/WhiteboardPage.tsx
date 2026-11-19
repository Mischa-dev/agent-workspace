// Before the canvas chunk loads: Excalidraw reads its font path as it starts.
import './assetPath'

import { Suspense, lazy, useEffect, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useParams } from 'react-router-dom'

import { callOp, newRequestId, opPrefix, opQuery } from '../api/ops'
import { toastError } from '../app/mutations'
import { useAuthenticatedSession } from '../auth/session'
import { RecentEditsBar, RecentEditsToggle } from '../changes/RecentEdits'
import { useRecentEdits } from '../changes/useRecentEdits'
import { AnchoredThreadPanel } from '../discussion/AnchoredThreadPanel'
import { useShellPage } from '../shell/shellStore'
import { ThingCommentsPanel } from '../discussion/ThingCommentsPanel'
import { useProjectExperience, useWorkspaceExperience } from '../experience/layout'
import { usePageTitle } from '../shell/documentTitleModel'
import { projectLocationHref } from '../projects/locations'
import { useTheme } from '../theme/themeContext'
import {
  AvatarStack,
  Banner,
  Button,
  ConfirmDialog,
  ErrorState,
  Icon,
  IconButton,
  Menu,
  PageSkeleton,
  SidePanel,
  StatusChip,
  actorKind,
  describeError,
  showToast,
} from '../ui'
import type { DraftHandle, SaveFailure } from './BoardCanvas'
import { BoardProposalsDialog } from './BoardProposals'
import { useBoardProposals } from './proposalModel'
import {
  deleteWhiteboardForever,
  duplicateWhiteboard,
  getScene,
  getWhiteboard,
  isDarkPreset,
  updateWhiteboard,
  whiteboardKeys,
  whiteboardPath,
  type Editor,
} from './api'
import { HistoryDialog, LinksDialog } from './BoardPanels'
import { AskDialog, BoardPins, SelectionActions } from './ElementComments'
import { anchorFor, pinnedThreads, sameIds, type BoardSelection, type BoardViewport, type ElementsAnchor } from './pins'
import { RenameDialog } from './dialogs'
import { saveLabel, type SaveState } from './sync'
import './whiteboards.css'

const BoardCanvas = lazy(() => import('./BoardCanvas'))

/** Someone counts as drawing now while their last change is this recent. */
const DRAWING_MS = 2 * 60_000

/** The time, read again every half minute, so "is drawing" fades once they stop. */
function useNow() {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000)
    return () => window.clearInterval(timer)
  }, [])
  return now
}

export function WhiteboardPage() {
  const { whiteboardId = '' } = useParams()
  const session = useAuthenticatedSession()
  useShellPage({ mainClassName: 'aw-main--whiteboard' })
  return (
    <Board key={whiteboardId} viewerId={session.user.actorId} whiteboardId={whiteboardId} />
  )
}

function Board({ viewerId, whiteboardId }: { viewerId: string; whiteboardId: string }) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const { preset } = useTheme()
  const detail = useQuery({ queryKey: whiteboardKeys.detail(whiteboardId), queryFn: () => getWhiteboard(whiteboardId) })
  // The scene loads once; after that the canvas keeps itself current.
  const scene = useQuery({
    queryKey: whiteboardKeys.scene(whiteboardId),
    queryFn: () => getScene(whiteboardId),
    staleTime: Infinity,
    gcTime: 0,
    refetchOnWindowFocus: false,
  })
  const [saveState, setSaveState] = useState<SaveState>('saved')
  const [editors, setEditors] = useState<Editor[]>([])
  const [panel, setPanel] = useState<'links' | 'history' | 'rename' | 'purge' | 'comments' | 'proposals' | null>(null)
  // Suggest mode (C1.11): what's drawn here waits until it's sent as a proposal.
  const draftRef = useRef<DraftHandle | null>(null)
  const [draftCount, setDraftCount] = useState(0)
  const [sending, setSending] = useState(false)
  const reviewable = Boolean(detail.data && (detail.data.canEdit || detail.data.canSuggest))
  const proposals = useBoardProposals(whiteboardId, reviewable)
  // A board in a project follows the project's layout; one outside any follows the workspace's.
  const projectExperience = useProjectExperience(detail.data?.whiteboard.projectId)
  const workspaceExperience = useWorkspaceExperience(detail.data?.whiteboard.projectId ? null : detail.data?.whiteboard.organizationId)
  const [saveFailure, setSaveFailure] = useState<SaveFailure | null>(null)
  const now = useNow()
  // Recent edits (C1.10): what others just drew is outlined in their name;
  // the toggle shows the last day's. A new drawing from anyone looks again.
  const [allEdits, setAllEdits] = useState(false)
  const lastDrawn = editors.reduce((latest, editor) => (editor.at > latest ? editor.at : latest), '')
  const showsRecent = Boolean(detail.data) && (detail.data?.whiteboard.projectId ? projectExperience : workspaceExperience).shows('page.recent_edits')
  const recent = useRecentEdits('whiteboard', whiteboardId, lastDrawn, { viewerId, all: allEdits, enabled: showsRecent })
  // Comments on elements (C1.31): pins where threads were started, and
  // comment or ask about what's selected.
  const commentsShown = Boolean(detail.data) && (detail.data?.whiteboard.projectId ? projectExperience : workspaceExperience).shows('thing.comments')
  const comments = useQuery({ ...opQuery('comment_list', { thing: { kind: 'whiteboard', id: whiteboardId } }), enabled: commentsShown })
  const [selection, setSelection] = useState<BoardSelection | null>(null)
  const [viewport, setViewport] = useState<BoardViewport | null>(null)
  const [commenting, setCommenting] = useState<{ anchor: ElementsAnchor; threadId?: string } | null>(null)
  const [asking, setAsking] = useState<BoardSelection | null>(null)
  usePageTitle(detail.data?.whiteboard.title ?? null)
  if (detail.isPending || scene.isPending) return <PageSkeleton cards={1} label="Loading whiteboard" />
  if (detail.isError || scene.isError || !detail.data || !scene.data) {
    return (
      <div className="aw-page">
        <ErrorState
          error={detail.error ?? scene.error}
          onRetry={() => void Promise.all([detail.refetch(), scene.refetch()])}
          title="This whiteboard didn’t open"
        />
      </div>
    )
  }
  const { whiteboard, canEdit, links } = detail.data
  const editable = canEdit && !whiteboard.deleted
  const suggesting = !canEdit && Boolean(detail.data.canSuggest) && !whiteboard.deleted
  const showComments = commentsShown
  const pinned = showComments && comments.data ? pinnedThreads(comments.data.threads) : []
  const commentingThread = !commenting || !comments.data
    ? null
    : (comments.data.threads.find((thread) =>
        commenting.threadId ? thread.id === commenting.threadId : pinned.some((entry) => entry.thread.id === thread.id && sameIds(entry.anchor.elementIds, commenting.anchor.elementIds)),
      ) ?? null)
  const commentOnSelection = () => {
    if (!selection) return
    const anchor = anchorFor(selection)
    const found = pinned.find((entry) => sameIds(entry.anchor.elementIds, anchor.elementIds))
    setCommenting({ anchor, threadId: found?.thread.id })
  }
  // Who else has drawn here lately; you're not in your own list.
  const others = (editors.length ? editors : scene.data.recentEditors).filter((editor) => editor.actorId && editor.actorId !== viewerId)
  const agents = others.filter((editor) => actorKind(editor.kind) === 'agent' && now - new Date(editor.at).getTime() < DRAWING_MS)
  const refresh = () => queryClient.invalidateQueries({ queryKey: whiteboardKeys.all })
  const trash = (deleted: boolean) =>
    updateWhiteboard(whiteboard.id, { deleted })
      .then(() => {
        void refresh()
        if (deleted) {
          showToast({ tone: 'success', title: `Moved “${whiteboard.title}” to the trash`, onUndo: () => trash(false) })
          navigate('/whiteboards')
        }
      })
      .catch((reason: unknown) =>
        toastError(deleted ? 'The whiteboard wasn’t moved to the trash' : 'The whiteboard wasn’t restored', reason),
      )
  const setTemplate = (isTemplate: boolean) =>
    updateWhiteboard(whiteboard.id, { isTemplate })
      .then(() => {
        void refresh()
        showToast({ tone: 'success', title: isTemplate ? 'New whiteboards can start from this one' : 'No longer a template' })
      })
      .catch((reason: unknown) => toastError('The whiteboard wasn’t changed', reason))
  const propose = async () => {
    const elements = draftRef.current?.draft() ?? []
    if (!elements.length) return
    setSending(true)
    try {
      const readVersions = draftRef.current?.readVersions() ?? {}
      const sent = await callOp('whiteboard_propose', { whiteboardId: whiteboard.id, elements, readVersions }, { requestId: newRequestId() })
      await draftRef.current?.discard()
      void queryClient.invalidateQueries({ queryKey: opPrefix('proposal_list') })
      showToast(
        sent.proposalId
          ? { tone: 'success', title: 'Sent for review', description: 'Someone who edits this board can accept or reject it.' }
          : { tone: 'info', title: 'Nothing to send: the board already looks like that' },
      )
    } catch (reason) {
      toastError('The proposal wasn’t sent', reason)
    } finally {
      setSending(false)
    }
  }
  const discard = () =>
    draftRef.current?.discard().catch((reason: unknown) => toastError('The board wasn’t reset', reason))
  // A board's notes become tasks, or the board a page, in its project (C1.32).
  const notesToTasks = (projectId: string) =>
    callOp('whiteboard_to_tasks', { whiteboardId: whiteboard.id }, { requestId: newRequestId() })
      .then((made) => {
        showToast({
          tone: 'success',
          title: made.tasks.length === 1 ? 'Made one task' : `Made ${made.tasks.length} tasks`,
          description: 'Each is labelled with its frame and links back here.',
          action: { label: 'Open tasks', onSelect: () => navigate(projectLocationHref(projectId, { kind: 'work_items' })) },
        })
      })
      .catch((reason: unknown) => toastError('No tasks were made', reason))
  const toPage = (projectId: string) =>
    callOp('page_make', { whiteboardId: whiteboard.id, spaceId: projectId }, { requestId: newRequestId() })
      .then((made) => navigate(projectLocationHref(made.spaceId, { kind: 'page', pageId: made.id })))
      .catch((reason: unknown) => toastError('The page wasn’t made', reason))
  const duplicate = () =>
    duplicateWhiteboard(whiteboard.id)
      .then((copy) => {
        void refresh()
        navigate(whiteboardPath(copy.whiteboard.id))
      })
      .catch((reason: unknown) => toastError('The whiteboard wasn’t copied', reason))
  return (
    <div className="aw-wb">
      <header className="aw-wb__bar">
        <Link className="aw-wb__back" to="/whiteboards">
          <Icon name="arrow-left" size={16} />
          <span>Whiteboards</span>
        </Link>
        <h1 className="aw-wb__title" id="workspace-heading">
          {whiteboard.title}
        </h1>
        <span className="aw-wb__place">
          <StatusChip icon={whiteboard.projectId ? 'lock' : 'users'} tone="muted">
            {whiteboard.projectName ?? whiteboard.organizationName}
          </StatusChip>
        </span>
        <span aria-live="polite" className="aw-wb__save" data-state={saveState}>
          {editable ? saveLabel(saveState) : suggesting ? (draftCount ? `${draftCount === 1 ? 'One change' : `${draftCount} changes`} not sent` : 'Suggesting') : 'View only'}
        </span>
        {suggesting && draftCount ? (
          <>
            <Button busy={sending} icon="send" onClick={() => void propose()} size="sm" variant="primary">
              Propose changes
            </Button>
            <Button disabled={sending} onClick={() => void discard()} size="sm" variant="ghost">
              Discard
            </Button>
          </>
        ) : null}
        <span className="aw-wb__spacer" />
        {agents.length ? (
          <StatusChip dots tone="agent">
            {agents.length === 1 ? `${agents[0].name} is drawing` : `${agents.length} agents are drawing`}
          </StatusChip>
        ) : null}
        {others.length ? (
          <span className="aw-wb__editors" title={`Recently edited by ${others.map((editor) => editor.name).join(', ')}`}>
            <AvatarStack
              people={others.map((editor) => ({ id: editor.actorId, name: editor.name, kind: actorKind(editor.kind) }))}
            />
            <span className="aw-sr-only">Recently edited by {others.map((editor) => editor.name).join(', ')}</span>
          </span>
        ) : null}
        {whiteboard.projectId && showsRecent ? <RecentEditsToggle on={allEdits} onChange={setAllEdits} /> : null}
        {proposals.length ? (
          <Button onClick={() => setPanel('proposals')} size="sm">
            {`Proposals (${proposals.length})`}
          </Button>
        ) : null}
        {showComments ? (
          <Button icon="messages" onClick={() => setPanel(panel === 'comments' ? null : 'comments')} size="sm">
            Comments
          </Button>
        ) : null}
        <Button icon="link" onClick={() => setPanel('links')} size="sm">
          {links.length ? `Linked (${links.length})` : 'Link'}
        </Button>
        <Menu
          items={[
            { label: 'Rename', icon: 'pencil', disabled: !editable, onSelect: () => setPanel('rename') },
            { label: 'History', icon: 'clock', onSelect: () => setPanel('history') },
            { label: 'Duplicate', icon: 'copy', onSelect: () => void duplicate() },
            ...(whiteboard.projectId && !whiteboard.deleted
              ? [
                  { label: 'Make tasks from notes', icon: 'checklist' as const, onSelect: () => void notesToTasks(whiteboard.projectId as string) },
                  { label: 'Make a page from board', icon: 'list' as const, onSelect: () => void toPage(whiteboard.projectId as string) },
                ]
              : []),
            {
              label: whiteboard.isTemplate ? 'Stop using as a template' : 'Use as a template',
              icon: 'copy',
              disabled: !editable,
              onSelect: () => void setTemplate(!whiteboard.isTemplate),
            },
            { separator: true },
            ...(whiteboard.deleted
              ? [
                  { label: 'Restore from trash', icon: 'undo' as const, disabled: !canEdit, onSelect: () => void trash(false) },
                  { label: 'Delete forever', icon: 'trash' as const, danger: true, disabled: !canEdit, onSelect: () => setPanel('purge') },
                ]
              : [{ label: 'Move to trash', icon: 'trash' as const, danger: true, disabled: !canEdit, onSelect: () => void trash(true) }]),
          ]}
          label="Whiteboard actions"
          placement="bottom-end"
          trigger={<IconButton icon="more" label="Whiteboard actions" size="sm" />}
        />
      </header>
      {whiteboard.deleted ? (
        <Banner
          action={
            canEdit ? (
              <>
                <Button icon="undo" onClick={() => void trash(false)} size="sm">
                  Restore it
                </Button>
                <Button icon="trash" onClick={() => setPanel('purge')} size="sm" variant="danger">
                  Delete forever
                </Button>
              </>
            ) : undefined
          }
          tone="offline"
          title="This whiteboard is in the trash."
        >
          {canEdit ? null : 'Ask someone who can edit it to restore it.'}
        </Banner>
      ) : null}
      {saveFailure && editable ? (
        <Banner onRetry={saveFailure.retry} tone="danger" title="Your latest changes aren’t saved.">
          {saveFailure.message} Keep this page open so they aren’t lost.
        </Banner>
      ) : null}
      {showsRecent && recent.newest ? (
        <div className="aw-wb__recent">
          <RecentEditsBar canUndo={editable} change={recent.newest} kind="whiteboard" />
        </div>
      ) : null}
      <div className="aw-wb__canvas">
        <Suspense fallback={<PageSkeleton cards={1} decorative label="Loading the canvas" />}>
          <BoardCanvas
            dark={isDarkPreset(preset)}
            draftRef={draftRef}
            editable={editable}
            edited={recent.shown}
            initial={scene.data}
            onDraft={setDraftCount}
            onEditors={setEditors}
            onSaveFailure={setSaveFailure}
            onSaveState={setSaveState}
            onSelection={setSelection}
            onViewport={setViewport}
            paused={sending}
            suggesting={suggesting}
            whiteboardId={whiteboard.id}
          />
        </Suspense>
        {showComments ? (
          <>
            <BoardPins onOpen={(entry) => setCommenting({ anchor: entry.anchor, threadId: entry.thread.id })} pinned={pinned} viewport={viewport} />
            <SelectionActions
              canAsk={!whiteboard.deleted && Boolean(comments.data?.acceptsComments)}
              onAsk={() => setAsking(selection)}
              onComment={commentOnSelection}
              selection={selection}
              viewport={viewport}
            />
          </>
        ) : null}
      </div>
      {showComments && panel === 'comments' ? (
        <SidePanel onClose={() => setPanel(null)} title="Comments">
          <ThingCommentsPanel kind="whiteboard" thingId={whiteboard.id} viewerId={viewerId} />
        </SidePanel>
      ) : null}
      {showComments && commenting && comments.data ? (
        <SidePanel
          onClose={() => setCommenting(null)}
          title={commenting.anchor.elementIds.length === 1 ? 'Comments on an element' : `Comments on ${commenting.anchor.elementIds.length} elements`}
        >
          <AnchoredThreadPanel
            acceptsComments={comments.data.acceptsComments}
            anchor={commenting.anchor}
            kind="whiteboard"
            onStarted={(threadId) => setCommenting((open) => (open ? { ...open, threadId } : open))}
            spaceId={comments.data.spaceId ?? whiteboard.projectId ?? ''}
            thingId={whiteboard.id}
            thread={commentingThread}
            viewerId={viewerId}
          />
        </SidePanel>
      ) : null}
      {asking ? (
        <AskDialog
          onAsked={(threadId) => {
            setAsking(null)
            setCommenting({ anchor: anchorFor(asking), threadId })
          }}
          onClose={() => setAsking(null)}
          selection={asking}
          spaceId={comments.data?.spaceId ?? whiteboard.projectId ?? ''}
          viewerId={viewerId}
          whiteboardId={whiteboard.id}
        />
      ) : null}
      {panel === 'proposals' ? (
        <BoardProposalsDialog canDecide={editable} onClose={() => setPanel(null)} proposals={proposals} userId={viewerId} />
      ) : null}
      <LinksDialog canEdit={canEdit} detail={detail.data} onClose={() => setPanel(null)} open={panel === 'links'} />
      <HistoryDialog canEdit={editable} detail={detail.data} onClose={() => setPanel(null)} open={panel === 'history'} />
      <RenameDialog board={whiteboard} onClose={() => setPanel(null)} open={panel === 'rename'} />
      {panel === 'purge' ? (
        <PurgeDialog
          id={whiteboard.id}
          onClose={() => setPanel(null)}
          onDeleted={() => {
            void refresh()
            navigate('/whiteboards')
          }}
          title={whiteboard.title}
        />
      ) : null}
    </div>
  )
}

/** Asks before a whiteboard in the trash goes for good. */
function PurgeDialog({ id, onClose, onDeleted, title }: { id: string; onClose: () => void; onDeleted: () => void; title: string }) {
  const purge = useMutation({
    mutationFn: () => deleteWhiteboardForever(id),
    onSuccess: () => {
      showToast({ tone: 'success', title: `Deleted “${title}” for good` })
      onDeleted()
    },
  })
  return (
    <ConfirmDialog
      busy={purge.isPending}
      confirmLabel="Delete forever"
      error={purge.error ? describeError(purge.error).message : null}
      onClose={onClose}
      onConfirm={() => purge.mutate()}
      open
      title={`Delete “${title}” forever?`}
    >
      Its drawing, images and history go for good, and it comes off every task, project and page it’s on. Copies stay.
    </ConfirmDialog>
  )
}
