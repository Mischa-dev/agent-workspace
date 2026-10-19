/**
 * The app's keyboard shortcuts: turning key presses into actions. Their
 * labels live in shortcutSheet.ts, loaded with the command menu; a test
 * presses every key the sheet shows, so the two can't drift apart.
 */

export type ShortcutAction =
  | 'command-menu'
  | 'shortcut-sheet'
  | 'create-task'
  | 'search'
  | 'mark-all-read'
  | 'previous-conversation'
  | 'next-conversation'
  | 'previous-unread'
  | 'next-unread'
  | 'go-home'
  | 'go-messages'
  | 'go-inbox'
  | 'go-my-work'
  | 'go-later'
  | 'go-tasks'
  | 'go-board'
  | 'go-pages'
  | 'go-decisions'
  | 'go-settings'
  | 'go-appearance'

/** G then a letter. The shortcut sheet (shortcutSheet.ts) lists these with labels. */
const G_SEQUENCES: Record<string, ShortcutAction> = {
  h: 'go-home',
  i: 'go-inbox',
  w: 'go-my-work',
  l: 'go-later',
  m: 'go-messages',
  t: 'go-tasks',
  b: 'go-board',
  p: 'go-pages',
  d: 'go-decisions',
  s: 'go-settings',
  a: 'go-appearance',
}

const SEQUENCE_TIMEOUT_MS = 1_200

type KeyLike = Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'target' | 'isComposing'>

/** Typing in a field, or a menu or dialog that has its own keys. */
export function isTypingTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false
  if (target.isContentEditable) return true
  if (target.closest('input, textarea, select, [contenteditable="true"], [role="textbox"], [role="combobox"]')) return true
  return Boolean(target.closest('[role="dialog"], [role="menu"], [role="listbox"]'))
}

/** A menu, listbox or dialog that has its own keys. */
function inOwnKeysRegion(target: EventTarget | null) {
  return target instanceof HTMLElement && Boolean(target.closest('[role="dialog"], [role="menu"], [role="listbox"]'))
}

/** The message composer, where the conversation keys work as they do in Slack. */
function inComposer(target: EventTarget | null) {
  return target instanceof HTMLElement && Boolean(target.closest('.aw-composer-wrap'))
}

/**
 * Turns key presses into shortcut actions. Single letters only count outside
 * text fields; ⌘K works everywhere, and the conversation keys (⌥↑, ⇧Esc)
 * work in the message composer too, as in Slack. In any other field ⌥↑ and
 * ⌥↓ keep moving the cursor by paragraph. Call `press` for each keydown.
 */
export function createShortcutMatcher(now: () => number = Date.now) {
  let pending: { key: string; at: number } | null = null

  return function press(event: KeyLike): ShortcutAction | null {
    if (event.isComposing) return null
    const key = event.key.length === 1 ? event.key.toLowerCase() : event.key
    if ((event.metaKey || event.ctrlKey) && !event.altKey && !event.shiftKey && key === 'k') {
      pending = null
      return 'command-menu'
    }
    const conversationKeys = !inOwnKeysRegion(event.target) && (!isTypingTarget(event.target) || inComposer(event.target))
    if (!event.metaKey && !event.ctrlKey && conversationKeys) {
      if (event.altKey && (key === 'ArrowUp' || key === 'ArrowDown')) {
        pending = null
        const up = key === 'ArrowUp'
        if (event.shiftKey) return up ? 'previous-unread' : 'next-unread'
        return up ? 'previous-conversation' : 'next-conversation'
      }
      if (!event.altKey && event.shiftKey && key === 'Escape') {
        pending = null
        return 'mark-all-read'
      }
    }
    if (event.metaKey || event.ctrlKey || event.altKey) return null
    if (isTypingTarget(event.target)) {
      pending = null
      return null
    }

    if (pending && now() - pending.at <= SEQUENCE_TIMEOUT_MS) {
      const first = pending.key
      pending = null
      const match = first === 'g' ? G_SEQUENCES[key] : undefined
      if (match) return match
    }
    pending = null

    if (event.key === '?') return 'shortcut-sheet'
    if (event.shiftKey) return null
    if (key === 'g') {
      pending = { key, at: now() }
      return null
    }
    if (key === 'c') return 'create-task'
    if (key === '/') return 'search'
    return null
  }
}

/** A task key such as "AWD-12": a project key, a dash and a number. */
export const TASK_KEY_PATTERN = /^([A-Za-z][A-Za-z0-9]{1,9})-([1-9][0-9]{0,8})$/

export function parseTaskKey(value: string): { projectKey: string; number: number; key: string } | null {
  const match = TASK_KEY_PATTERN.exec(value.trim())
  if (!match) return null
  const projectKey = match[1].toUpperCase()
  const number = Number(match[2])
  return { projectKey, number, key: `${projectKey}-${number}` }
}
