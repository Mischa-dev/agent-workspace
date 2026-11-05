import type { OpenTask } from './TaskConnections'
import { TaskDetail } from './TaskDetail'
import './work.css'

/**
 * A task in the side panel beside a list, as in Tasks and My work. Escape
 * anywhere in it closes it, unless a field or a popover in it wants the key.
 */
export function TaskPanel({
  onClose,
  onOpenTask,
  projectArchived,
  projectId,
  taskId,
}: {
  onClose: () => void
  onOpenTask: OpenTask
  projectArchived: boolean
  projectId: string
  taskId: string
}) {
  return (
    <div
      className="aw-taskpanel"
      onKeyDown={(event) => {
        if (event.key !== 'Escape' || event.defaultPrevented) return
        if ((event.target as HTMLElement).closest('input, textarea, select, .aw-popover, [role="dialog"][aria-modal="true"]:not(.aw-panel)')) return
        event.preventDefault()
        onClose()
      }}
    >
      <TaskDetail
        key={taskId}
        layout="panel"
        onClose={onClose}
        onOpenTask={onOpenTask}
        projectArchived={projectArchived}
        projectId={projectId}
        taskId={taskId}
      />
    </div>
  )
}
