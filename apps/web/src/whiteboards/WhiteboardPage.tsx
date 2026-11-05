// Before the canvas chunk loads: Excalidraw reads its font path as it starts.
import './assetPath'

import { Suspense, lazy, useEffect, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useParams } from 'react-router-dom'

import { toastError } from '../app/mutations'
import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { usePageTitle } from '../shell/documentTitleModel'
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
  StatusChip,
  actorKind,
  describeError,
  showToast,
} from '../ui'
import type { SaveFailure } from './BoardCanvas'
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
  return (
    <AppShell mainClassName="aw-main--whiteboard" user={session.user}>
      <Board key={whiteboardId} viewerId={session.user.actorId} whiteboardId={whiteboardId} />
    </AppShell>
  )
}

function Board({ viewerId, whiteboardId }: { viewerId: string; whiteboardId: string }) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const { preset } = useTheme()
  const detail = useQuery({ queryKey: whiteboardKeys.detail(whiteboardId), queryFn: () => getWhiteboard(whiteboardId) })
  // The scene loads once; after that the canvas keeps itself current.
  const scene = useQuery({
    queryKey: ['whiteboard-scene', whiteboardId],
    queryFn: () => getScene(whiteboardId),
    staleTime: Infinity,
    gcTime: 0,
    refetchOnWindowFocus: false,
  })
  const [saveState, setSaveState] = useState<SaveState>('saved')
  const [editors, setEditors] = useState<Editor[]>([])
  const [panel, setPanel] = useState<'links' | 'history' | 'rename' | 'purge' | null>(null)
  const [saveFailure, setSaveFailure] = useState<SaveFailure | null>(null)
  const now = useNow()
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
          {editable ? saveLabel(saveState) : 'View only'}
        </span>
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
        <Button icon="link" onClick={() => setPanel('links')} size="sm">
          {links.length ? `Linked (${links.length})` : 'Link'}
        </Button>
        <Menu
          items={[
            { label: 'Rename', icon: 'pencil', disabled: !editable, onSelect: () => setPanel('rename') },
            { label: 'History', icon: 'clock', onSelect: () => setPanel('history') },
            { label: 'Duplicate', icon: 'copy', onSelect: () => void duplicate() },
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
      <div className="aw-wb__canvas">
        <Suspense fallback={<PageSkeleton cards={1} decorative label="Loading the canvas" />}>
          <BoardCanvas
            dark={isDarkPreset(preset)}
            editable={editable}
            initial={scene.data}
            onEditors={setEditors}
            onSaveFailure={setSaveFailure}
            onSaveState={setSaveState}
            whiteboardId={whiteboard.id}
          />
        </Suspense>
      </div>
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
