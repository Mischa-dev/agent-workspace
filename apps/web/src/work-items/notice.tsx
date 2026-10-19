import { showToast, type ToastTone } from '../ui'

/** What the list and board say after a change: a toast, with Undo or a follow-up when there is one. */
export type Notice = {
  /** Stands alone, so no full stop. */
  title: string
  description?: string
  tone: Exclude<ToastTone, 'agent'>
  action?: { label: string; run: () => void }
  onUndo?: () => void | Promise<unknown>
}

export function notify(notice: Notice) {
  showToast({
    title: notice.title,
    description: notice.description,
    tone: notice.tone,
    action: notice.action ? { label: notice.action.label, onSelect: notice.action.run } : undefined,
    onUndo: notice.onUndo,
  })
}
