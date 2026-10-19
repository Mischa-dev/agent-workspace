import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Pin as PinIcon } from 'lucide-react'
import { Link } from 'react-router-dom'

import { toastError } from '../app/mutations'
import { Icon, IconButton, plainPreview, showToast } from '../ui'
import { memoryKeys, pin, unpin, type Pin } from './api'
import { usePins } from './hooks'

/** Pin or unpin, then say so, with Undo; a failure says why. */
function usePinActions(projectId: string, conversationId: string) {
  const queryClient = useQueryClient()
  const refresh = () => queryClient.invalidateQueries({ queryKey: memoryKeys.pins(projectId, conversationId) })
  const setPinned = (kind: Pin['kind'], id: string, pinned: boolean) => (pinned ? pin(conversationId, kind, id) : unpin(conversationId, kind, id))
  const change = (kind: Pin['kind'], id: string, pinned: boolean) => {
    void setPinned(kind, id, pinned)
      .then(async () => {
        await refresh()
        showToast({
          title: pinned ? 'Pinned to this conversation' : 'Unpinned',
          onUndo: () =>
            void setPinned(kind, id, !pinned)
              .then(refresh)
              .catch((error: unknown) => toastError('It wasn’t undone', error)),
        })
      })
      .catch((error: unknown) => toastError(pinned ? 'It wasn’t pinned' : 'It wasn’t unpinned', error))
  }
  return change
}

/** What's pinned in a conversation: the pages, decisions and messages that are true here now. */
export function PinnedBar({
  canEdit,
  conversationId,
  direct = false,
  projectId,
}: {
  canEdit: boolean
  conversationId: string
  /** DMs and groups have no project pages to link to. */
  direct?: boolean
  projectId: string
}) {
  const pins = usePins(projectId, conversationId)
  const change = usePinActions(projectId, conversationId)
  const [open, setOpen] = useState(false)
  const items = pins.data?.pins ?? []
  if (items.length === 0) return null
  const href = (item: Pin) =>
    item.kind === 'page'
      ? `/projects/${encodeURIComponent(projectId)}/pages/${encodeURIComponent(item.id)}`
      : item.kind === 'decision'
        ? `/projects/${encodeURIComponent(projectId)}/decisions#${encodeURIComponent(item.id)}`
        : `#message-${item.id}`
  return (
    <div className="aw-pins">
      <button aria-expanded={open} className="aw-pins__toggle" onClick={() => setOpen(!open)} type="button">
        <Icon name={PinIcon} size={14} />
        {items.length.toLocaleString()} pinned
        <Icon name="chevron-down" size={12} />
      </button>
      {open ? (
        <ul className="aw-pins__list">
          {items.map((item) => (
            <li className="aw-pins__item" key={`${item.kind}:${item.id}`}>
              {item.kind === 'message' ? (
                <a className="aw-pins__title" href={href(item)}>
                  {plainPreview(item.title, 120)}
                </a>
              ) : direct ? (
                <strong className="aw-pins__title">{item.title}</strong>
              ) : (
                <Link className="aw-pins__title" to={href(item)}>
                  {item.title}
                </Link>
              )}
              <span className="aw-pins__text">{plainPreview(item.text, 160)}</span>
              {canEdit ? <IconButton icon="x" label={`Unpin ${plainPreview(item.title, 60)}`} onClick={() => change(item.kind, item.id, false)} size="sm" /> : null}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  )
}
