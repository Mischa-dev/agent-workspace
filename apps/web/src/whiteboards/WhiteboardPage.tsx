// Before the canvas chunk loads: Excalidraw reads its font path as it starts.
import './assetPath'

import { Suspense, lazy, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useParams } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { useTheme } from '../theme/themeContext'
import {
  AvatarStack,
  Banner,
  Button,
  ErrorState,
  Icon,
  IconButton,
  Menu,
  PageSkeleton,
  StatusChip,
  actorKind,
  showToast,
} from '../ui'
import {
  duplicateWhiteboard,
  getScene,
  getWhiteboard,
  updateWhiteboard,
  whiteboardKeys,
  whiteboardPath,
  type Editor,
} from './api'
import { HistoryDialog, LinksDialog } from './BoardPanels'
import { RenameDialog } from './dialogs'
import { errorText } from './errors'
import { saveLabel, type SaveState } from './sync'
import './whiteboards.css'

const BoardCanvas = lazy(() => import('./BoardCanvas'))

export function WhiteboardPage() {
  const { whiteboardId = '' } = useParams()
  const session = useAuthenticatedSession()
  return (
    <AppShell mainClassName="aw-main--whiteboard" user={session.user}>
      <Board key={whiteboardId} whiteboardId={whiteboardId} />
    </AppShell>
  )
}

function Board({ whiteboardId }: { whiteboardId: string }) {
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
  const [panel, setPanel] = useState<'links' | 'history' | 'rename' | null>(null)
  if (detail.isPending || scene.isPending) return <PageSkeleton cards={1} label="Loading whiteboard" />
  if (detail.isError || scene.isError || !detail.data || !scene.data) {
    return (
      <div className="aw-page">
        <ErrorState
          action={
            <Button icon="retry" onClick={() => void Promise.all([detail.refetch(), scene.refetch()])}>
              Try again
            </Button>
          }
          title="This whiteboard didn’t open"
        >
          {errorText(detail.error ?? scene.error, 'It may have been removed, or you may not have access to it.')}
        </ErrorState>
      </div>
    )
  }
  const { whiteboard, canEdit, links } = detail.data
  const editable = canEdit && !whiteboard.deleted
  const others = (editors.length ? editors : scene.data.recentEditors).filter((editor) => editor.actorId)
  const agents = others.filter((editor) => actorKind(editor.kind) === 'agent')
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
      .catch((reason: unknown) => showToast({ tone: 'danger', title: errorText(reason, 'That didn’t work.') }))
  const duplicate = () =>
    duplicateWhiteboard(whiteboard.id)
      .then((copy) => {
        void refresh()
        navigate(whiteboardPath(copy.whiteboard.id))
      })
      .catch((reason: unknown) => showToast({ tone: 'danger', title: errorText(reason, 'The board wasn’t copied.') }))
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
            whiteboard.deleted
              ? { label: 'Restore from trash', icon: 'undo', disabled: !canEdit, onSelect: () => void trash(false) }
              : { label: 'Move to trash', icon: 'trash', danger: true, disabled: !canEdit, onSelect: () => void trash(true) },
          ]}
          label="Whiteboard actions"
          placement="bottom-end"
          trigger={<IconButton icon="more" label="Whiteboard actions" size="sm" />}
        />
      </header>
      {whiteboard.deleted ? (
        <Banner tone="offline" title="This board is in the trash.">
          {canEdit ? (
            <Button onClick={() => void trash(false)} size="sm">
              Restore it
            </Button>
          ) : (
            'Ask someone who can edit it to restore it.'
          )}
        </Banner>
      ) : null}
      <div className="aw-wb__canvas">
        <Suspense fallback={<PageSkeleton cards={1} decorative label="Loading the canvas" />}>
          <BoardCanvas
            dark={preset === 'midnight'}
            editable={editable}
            initial={scene.data}
            onEditors={setEditors}
            onSaveState={setSaveState}
            whiteboardId={whiteboard.id}
          />
        </Suspense>
      </div>
      <LinksDialog canEdit={canEdit} detail={detail.data} onClose={() => setPanel(null)} open={panel === 'links'} />
      <HistoryDialog canEdit={editable} detail={detail.data} onClose={() => setPanel(null)} open={panel === 'history'} />
      <RenameDialog board={whiteboard} onClose={() => setPanel(null)} open={panel === 'rename'} />
    </div>
  )
}
