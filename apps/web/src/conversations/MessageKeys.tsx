import { useEffect, useRef } from 'react'

/**
 * Pressing R, E or S on a focused message presses its toolbar button marked
 * with that key (`data-message-key`); T opens "Create task", which lives in
 * the More menu. R with no thread button quotes the message in the box
 * below. Only the message itself counts as focused, never a button or field
 * inside it, and a handled key never reaches the app's own shortcuts.
 */
export function MessageKeys({ onReply, onTask }: { onReply?: () => void; onTask?: () => void }) {
  const marker = useRef<HTMLSpanElement>(null)
  const actions = useRef({ onReply, onTask })
  useEffect(() => {
    actions.current = { onReply, onTask }
  })
  useEffect(() => {
    const message = marker.current?.closest<HTMLElement>('.aw-msg')
    if (!message) return
    const onKey = (event: KeyboardEvent) => {
      if (event.target !== message || event.defaultPrevented) return
      if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey || event.isComposing) return
      const key = event.key.toLowerCase()
      if (!['r', 'e', 's', 't'].includes(key)) return
      const button = message.querySelector<HTMLElement>(`[data-message-key="${key}"]`)
      let handled = false
      if (button) {
        button.click()
        handled = true
      } else if (key === 'r' && actions.current.onReply) {
        actions.current.onReply()
        handled = true
      } else if (key === 't' && actions.current.onTask) {
        actions.current.onTask()
        handled = true
      }
      if (handled) event.preventDefault()
    }
    message.addEventListener('keydown', onKey)
    return () => message.removeEventListener('keydown', onKey)
  }, [])
  return <span hidden ref={marker} />
}
