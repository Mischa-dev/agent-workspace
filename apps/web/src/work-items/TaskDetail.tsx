import { Children, Fragment, lazy, Suspense, useRef, useState, type DragEvent, type ReactNode } from 'react'
import { Maximize2 } from 'lucide-react'
import { Link } from 'react-router-dom'

import {
  Button,
  DocumentSkeleton,
  ErrorState,
  Icon,
  IconButton,
  Menu,
  StatusChip,
  Tooltip,
  formatFullDate,
  formatRelative,
  showToast,
  type MenuEntry,
} from '../ui'
import { buildOpenInCodexLink } from './codexDeepLink'
import { useTaskFiles, useWorkItem, useWorkItemWorkflow } from './hooks'
import { workItemHref, workItemsHref } from './links'
import { LabelManager } from './LabelManager'
import { TaskActivity } from './TaskActivity'
import { DependencyMenu, TaskDependencies, TaskSubtasks, type DependencyRequest, type OpenTask } from './TaskConnections'
import { AcceptanceCriteria } from './TaskCriteria'
import { TaskDescription } from './TaskDescription'
import { isFinished } from './taskDisplay'
import { TitleField } from './TaskFields'
import { TaskFiles, type TaskFilesHandle } from './TaskFiles'
import { TaskGlyph } from './TaskGlyph'
import { TaskNotice } from './TaskNotice'
import { BlockForm, BlockedNotice, ClaimNotice, HandoffNotice } from './TaskNotes'
import { useArchive, useBoardCount, useEditShortcut, useWide } from './taskPageHooks'
import { TaskProperties } from './TaskProperties'
import { TaskSearchPicker } from './TaskSearchPicker'
import { TaskStart, type StartFix } from './TaskStart'
import type { WorkItem, WorkItemWorkflow } from './types'
import { useTaskSave } from './useTaskSave'

export type TaskLayout = 'panel' | 'page'

// Suggestions and summaries load in their own chunk.
const TaskAiPanel = lazy(() => import('../ai/TaskAiPanel'))
// The boards on a task load in the whiteboards' own small chunk.
const LinkedWhiteboards = lazy(() =>
  import('../whiteboards/LinkedWhiteboards').then((module) => ({ default: module.LinkedWhiteboards })),
)
// Links load the first time a task has one.
const TaskLinks = lazy(() => import('./TaskLinks').then((module) => ({ default: module.TaskLinks })))
// What to do about a save that stopped loads only when one does.
const TaskSaveNotices = lazy(() => import('./TaskSaveNotices').then((module) => ({ default: module.TaskSaveNotices })))

/**
 * A task, loaded by id: the side panel on the list and board, and the full
 * page. Every field saves on its own as it changes.
 */
export function TaskDetail({
  layout,
  onClose,
  onOpenTask,
  projectArchived,
  projectId,
  taskId,
}: {
  layout: TaskLayout
  onClose?: () => void
  onOpenTask: OpenTask
  projectArchived: boolean
  projectId: string
  taskId: string
}) {
  const item = useWorkItem(projectId, taskId)
  const workflow = useWorkItemWorkflow(projectId)
  if (item.isPending || workflow.isPending) {
    return (
      <TaskFrame layout={layout} onClose={onClose} projectId={projectId}>
        <DocumentSkeleton label="Loading task" paragraphs={2} />
      </TaskFrame>
    )
  }
  if (item.isError || workflow.isError) {
    return (
      <TaskFrame layout={layout} onClose={onClose} projectId={projectId}>
        <ErrorState
          action={<Button onClick={() => void Promise.all([item.refetch(), workflow.refetch()])}>Try again</Button>}
          title="This task isn’t available"
        >
          It may have been removed, or your access may have changed. If your connection dropped, try again.
        </ErrorState>
      </TaskFrame>
    )
  }
  return (
    <TaskDetailBody
      item={item.data}
      key={item.data.id}
      layout={layout}
      onClose={onClose}
      onOpenTask={onOpenTask}
      projectArchived={projectArchived}
      workflow={workflow.data}
    />
  )
}

function TaskFrame({ children, layout, onClose, projectId }: { children: ReactNode; layout: TaskLayout; onClose?: () => void; projectId: string }) {
  return (
    <div className={`aw-task aw-task--${layout}`}>
      <TaskBar layout={layout} onClose={onClose} projectId={projectId} />
      <div className="aw-task__body">
        <div className="aw-task__column">{children}</div>
      </div>
    </div>
  )
}

/** The bar along the top: where the task is, then its actions. It stays put while the task scrolls. */
function TaskBar({
  children,
  item,
  layout,
  onClose,
  projectId,
}: {
  children?: ReactNode
  item?: WorkItem
  layout: TaskLayout
  onClose?: () => void
  projectId: string
}) {
  return (
    <div className="aw-task__bar">
      {layout === 'page' ? (
        <nav aria-label="Breadcrumb" className="aw-crumbs">
          <Link className="aw-crumbs__link" to={workItemsHref(projectId)}>
            <Icon name="work" size={14} />
            Tasks
          </Link>
          <Icon className="aw-crumbs__sep" name="chevron-right" size={14} />
          <span aria-current="page" className="aw-crumbs__here">
            {item?.key ?? 'Task'}
          </span>
        </nav>
      ) : item ? (
        <span className="aw-task__key">{item.key}</span>
      ) : null}
      {item?.archivedAt ? <StatusChip icon="archive">Archived</StatusChip> : null}
      <span className="aw-task__spacer" />
      {children}
      {onClose ? <IconButton icon="x" label="Close panel" onClick={onClose} shortcut="Esc" side="bottom" /> : null}
    </div>
  )
}

type Adding = 'subtask' | 'criterion' | 'link' | null

// Opening something from a menu waits a frame, until the menu has handed focus back to its button.
const afterMenu = (open: () => void) => {
  window.requestAnimationFrame(open)
}

function TaskDetailBody({
  item,
  layout,
  onClose,
  onOpenTask,
  projectArchived,
  workflow,
}: {
  item: WorkItem
  layout: TaskLayout
  onClose?: () => void
  onOpenTask: OpenTask
  projectArchived: boolean
  workflow: WorkItemWorkflow
}) {
  const save = useTaskSave(item)
  const disabled = projectArchived || Boolean(item.archivedAt) || !item.capabilities.edit
  const root = useRef<HTMLDivElement>(null)
  const wide = useWide(root, layout === 'page')
  const editDescription = useRef<(() => void) | null>(null)
  const files = useRef<TaskFilesHandle | null>(null)
  const [menuAnchor, setMenuAnchor] = useState<HTMLSpanElement | null>(null)
  const taskFiles = useTaskFiles(item.projectId, item.id)
  const boards = useBoardCount(item.id)
  const archive = useArchive(item, projectArchived)
  const [adding, setAdding] = useState<Adding>(null)
  const [dependency, setDependency] = useState<DependencyRequest | null>(null)
  const [parentAnchor, setParentAnchor] = useState<HTMLElement | null>(null)
  const [blocking, setBlocking] = useState(false)
  const [boardDialog, setBoardDialog] = useState<'new' | 'link' | null>(null)
  const [managingLabels, setManagingLabels] = useState(false)
  const [dropping, setDropping] = useState(false)
  useEditShortcut(!disabled, editDescription, layout)

  const hasLinks = item.linkedThreads.length > 0 || item.linkedRepositories.length > 0 || item.linkedPullRequests.length > 0
  const ownFiles = (taskFiles.data ?? []).filter((file) => !file.commentId)
  const finished = isFinished(item)
  const titleSave = save.pending('title')
  const saveStopped = save.stopped.length > 0 || Boolean(save.problem) || (titleSave !== null && titleSave.state !== 'saving')

  const fix = (what: StartFix) => {
    if (what === 'description') editDescription.current?.()
    else setAdding(what)
  }

  const copy = (text: string, done: string) => {
    void navigator.clipboard
      ?.writeText(text)
      .then(() => showToast({ tone: 'success', title: done }))
      .catch(() => showToast({ tone: 'danger', title: 'That didn’t copy. Try again.' }))
  }

  const openInCodex = () => {
    const link = projectArchived
      ? { href: null, reason: 'Restore this project before opening its tasks in Codex.' }
      : buildOpenInCodexLink(item, window.location.origin)
    if (link.href) window.location.assign(link.href)
    else showToast({ title: 'This task can’t open in Codex yet.', description: link.reason ?? undefined, duration: 8_000 })
  }

  const menu: MenuEntry[] = [
    { label: 'Copy link', icon: 'link', onSelect: () => copy(`${window.location.origin}${workItemHref(item.projectId, item.id)}`, 'Link copied') },
    { label: `Copy ${item.key}`, icon: 'copy', onSelect: () => copy(item.key, `${item.key} copied`) },
  ]
  if (!disabled) {
    menu.push(
      { separator: true },
      {
        label: item.parent ? 'Change parent…' : 'Set parent…',
        icon: 'branch',
        onSelect: () => afterMenu(() => setParentAnchor(menuAnchor)),
      },
      item.blocked
        ? { label: 'Unblock', icon: 'circle-check', onSelect: () => save.save('blocked', { blockedReason: null }) }
        : { label: 'Mark as blocked…', icon: 'circle-minus', onSelect: () => afterMenu(() => setBlocking(true)) },
    )
  }
  menu.push({ separator: true }, { label: 'Open in Codex', icon: 'terminal', onSelect: openInCodex })
  if (archive.allowed) {
    menu.push(
      { separator: true },
      item.archivedAt
        ? { label: 'Restore task', icon: 'undo', onSelect: archive.restore }
        : { label: 'Archive task', icon: 'archive', onSelect: archive.archive },
    )
  }

  const bar = (
    <TaskBar item={item} layout={layout} onClose={onClose} projectId={item.projectId}>
      <span aria-live="polite" className="aw-task__saving">
        {save.saving ? 'Saving…' : ''}
      </span>
      {layout === 'panel' ? (
        <Tooltip label="Open full page" side="bottom">
          <Link aria-label="Open full page" className="aw-iconbtn aw-iconbtn--md" to={workItemHref(item.projectId, item.id)}>
            <Icon name={Maximize2} size={16} />
          </Link>
        </Tooltip>
      ) : null}
      <span className="aw-menuanchor" ref={setMenuAnchor}>
        <Menu items={menu} label="Task actions" placement="bottom-end" trigger={<IconButton icon="more" label="Task actions" side="bottom" />} />
      </span>
    </TaskBar>
  )

  const notices = (
    <div className="aw-task__notices">
      {projectArchived && layout === 'page' ? (
        <TaskNotice icon="archive" title="This project is archived, so its tasks can’t change." tone="muted" />
      ) : null}
      {item.archivedAt ? (
        <TaskNotice
          actions={
            archive.allowed ? (
              <Button busy={archive.busy} icon="undo" onClick={archive.restore} size="sm">
                Restore
              </Button>
            ) : undefined
          }
          icon="archive"
          title="This task is archived."
          tone="muted"
        >
          It’s hidden from lists and can’t change until it’s restored.
        </TaskNotice>
      ) : null}
      {saveStopped ? (
        <Suspense fallback={null}>
          <TaskSaveNotices item={item} save={save} />
        </Suspense>
      ) : null}
      <ClaimNotice item={item} projectArchived={projectArchived} />
      {item.blocked ? (
        <BlockedNotice blocked={item.blocked} onUnblock={disabled ? undefined : () => save.save('blocked', { blockedReason: null })} />
      ) : null}
      {blocking && !item.blocked && !disabled ? (
        <BlockForm
          onCancel={() => setBlocking(false)}
          onSubmit={(reason) => {
            setBlocking(false)
            save.save('blocked', { blockedReason: reason })
          }}
        />
      ) : null}
      {item.handoff && !finished ? <HandoffNotice handoff={item.handoff} /> : null}
    </div>
  )

  const addRow = disabled ? null : (
    <AddRow>
      {!item.subtasks.length && adding !== 'subtask' ? (
        <button aria-label="Add a subtask" className="aw-taskadd__item" onClick={() => setAdding('subtask')} type="button">
          subtask
        </button>
      ) : null}
      {!item.blockedBy.length && !item.blocks.length && !item.relations.length ? (
        <DependencyMenu
          onChoose={setDependency}
          trigger={
            <button aria-label="Add a dependency" className="aw-taskadd__item" type="button">
              dependency
            </button>
          }
        />
      ) : null}
      {!item.acceptanceCriteria.length && adding !== 'criterion' ? (
        <button aria-label="Add an acceptance criterion" className="aw-taskadd__item" onClick={() => setAdding('criterion')} type="button">
          criterion
        </button>
      ) : null}
      {!hasLinks && adding !== 'link' ? (
        <button aria-label="Add a link" className="aw-taskadd__item" onClick={() => setAdding('link')} type="button">
          link
        </button>
      ) : null}
      {taskFiles.isSuccess && !ownFiles.length ? (
        <button aria-label="Attach a file" className="aw-taskadd__item" onClick={() => files.current?.choose()} type="button">
          file
        </button>
      ) : null}
      {boards === 0 ? (
        <Menu
          items={[
            { label: 'New whiteboard…', icon: 'plus', onSelect: () => afterMenu(() => setBoardDialog('new')) },
            { label: 'Link a whiteboard…', icon: 'link', onSelect: () => afterMenu(() => setBoardDialog('link')) },
          ]}
          label="Add a whiteboard"
          trigger={
            <button aria-label="Add a whiteboard" className="aw-taskadd__item" type="button">
              whiteboard
            </button>
          }
        />
      ) : null}
    </AddRow>
  )

  const properties = (
    <TaskProperties
      disabled={disabled}
      item={item}
      onManageLabels={() => setManagingLabels(true)}
      save={save}
      variant={wide ? 'rail' : 'pills'}
      workflow={workflow}
    />
  )
  const start = <TaskStart disabled={disabled} item={item} onFix={fix} onOpenTask={onOpenTask} />

  const onDragOver = (event: DragEvent<HTMLDivElement>) => {
    if (disabled || event.defaultPrevented || !event.dataTransfer.types.includes('Files')) return
    event.preventDefault()
    if (!dropping) setDropping(true)
  }
  const onDrop = (event: DragEvent<HTMLDivElement>) => {
    setDropping(false)
    // The comment box takes files dropped on it for the comment.
    if (disabled || event.defaultPrevented || !event.dataTransfer.files.length) return
    event.preventDefault()
    files.current?.add(event.dataTransfer.files)
  }

  return (
    <div
      className={`aw-task aw-task--${layout}${wide ? ' is-wide' : ''}${dropping ? ' is-dropping' : ''}`}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDropping(false)
      }}
      onDragOver={onDragOver}
      onDrop={onDrop}
      ref={root}
    >
      {bar}
      <div className="aw-task__body">
        <div className="aw-task__column">
          <div className="aw-task__head">
            <h1 className="visually-hidden" id={layout === 'page' ? 'workspace-heading' : undefined}>
              {item.key}: {item.title}
            </h1>
            {item.parent ? <ParentLine onOpenTask={onOpenTask} parent={item.parent} /> : null}
            <TitleField disabled={disabled} item={item} save={save} />
            {wide ? null : properties}
          </div>
          {notices}
          <div className="aw-task__text">
            <TaskDescription disabled={disabled} editRef={editDescription} item={item} save={save} />
            {addRow}
          </div>
          <TaskSubtasks
            adding={adding === 'subtask'}
            disabled={disabled}
            item={item}
            onAdding={(on) => setAdding(on ? 'subtask' : null)}
            onOpen={onOpenTask}
          />
          <TaskDependencies disabled={disabled} item={item} onOpen={onOpenTask} onRequest={setDependency} request={dependency} save={save} />
          <AcceptanceCriteria adding={adding === 'criterion'} disabled={disabled} item={item} onAdding={(on) => setAdding(on ? 'criterion' : null)} />
          {hasLinks || adding === 'link' ? (
            <Suspense fallback={null}>
              <TaskLinks adding={adding === 'link'} disabled={disabled} item={item} onAdding={(on) => setAdding(on ? 'link' : null)} />
            </Suspense>
          ) : null}
          <TaskFiles disabled={disabled} handle={files} item={item} />
          <Suspense fallback={null}>
            <LinkedWhiteboards
              canEdit={!disabled}
              dialog={boardDialog}
              hideWhenEmpty
              onDialog={setBoardDialog}
              projectId={item.projectId}
              targetId={item.id}
              targetKind="task"
            />
          </Suspense>
          {wide ? null : start}
          <Suspense fallback={null}>
            <TaskAiPanel disabled={disabled} projectId={item.projectId} taskId={item.id} />
          </Suspense>
          <TaskActivity disabled={disabled} item={item} projectArchived={projectArchived} />
        </div>
        {wide ? (
          <aside aria-label="Task details" className="aw-task__rail">
            {properties}
            {start}
            <p className="aw-task__dates">
              Created{' '}
              <time dateTime={item.createdAt} title={formatFullDate(item.createdAt)}>
                {formatRelative(item.createdAt)}
              </time>
              {' · '}Updated{' '}
              <time dateTime={item.updatedAt} title={formatFullDate(item.updatedAt)}>
                {formatRelative(item.updatedAt)}
              </time>
            </p>
          </aside>
        ) : null}
      </div>
      {dropping ? (
        <p aria-hidden="true" className="aw-task__drop">
          Drop to attach to {item.key}
        </p>
      ) : null}
      <TaskSearchPicker
        allowTypedKey={false}
        anchor={parentAnchor}
        exclude={[item.id, ...item.subtasks.map((task) => task.id)]}
        label="Set parent"
        noneLabel={item.parent ? 'No parent' : undefined}
        onClose={() => setParentAnchor(null)}
        onPick={(target) => {
          setParentAnchor(null)
          save.save('parent', { parentWorkItemId: target.id })
        }}
        projectId={item.projectId}
      />
      {managingLabels ? <LabelManager onClose={() => setManagingLabels(false)} projectId={item.projectId} /> : null}
    </div>
  )
}

/** "Subtask of AW-3 Ship the export", above the title. */
function ParentLine({ onOpenTask, parent }: { onOpenTask: OpenTask; parent: NonNullable<WorkItem['parent']> }) {
  return (
    <p className="aw-task__parent">
      <TaskGlyph name="subtasks" />
      Subtask of{' '}
      <a
        className="aw-task__parentlink"
        href={parent.href}
        onClick={(event) => {
          if (event.metaKey || event.ctrlKey || event.shiftKey) return
          event.preventDefault()
          onOpenTask(parent)
        }}
      >
        <span className="aw-task__parentkey">{parent.key}</span> {parent.title}
      </a>
    </p>
  )
}

/** The things a task doesn't have yet, as one row of ways to add them. The sections appear once there's something in them. */
function AddRow({ children }: { children: ReactNode }) {
  const items = Children.toArray(children)
  if (!items.length) return null
  return (
    <div aria-label="Add to this task" className="aw-taskadd" role="group">
      <span aria-hidden="true" className="aw-taskadd__label">
        Add
      </span>
      {items.map((child, index) => (
        <Fragment key={index}>
          {index ? (
            <span aria-hidden="true" className="aw-taskadd__dot">
              ·
            </span>
          ) : null}
          {child}
        </Fragment>
      ))}
    </div>
  )
}
