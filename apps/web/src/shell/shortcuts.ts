/**
 * The app's keyboard shortcuts. One list drives both the key handling and the
 * shortcut sheet, so the sheet can't drift from what the keys do.
 */

export type ShortcutAction =
  | 'command-menu'
  | 'shortcut-sheet'
  | 'create-task'
  | 'search'
  | 'go-home'
  | 'go-messages'
  | 'go-inbox'
  | 'go-tasks'
  | 'go-board'
  | 'go-pages'
  | 'go-decisions'
  | 'go-settings'
  | 'go-appearance'

export type Shortcut = {
  action: ShortcutAction
  /** The keys as shown: ["⌘", "K"], or ["G", "I"] for a sequence. */
  keys: string[]
  label: string
  group: 'General' | 'Go to'
  /** Needs a Project open. */
  inProject?: boolean
}

export const SHORTCUTS: Shortcut[] = [
  { action: 'command-menu', keys: ['⌘', 'K'], label: 'Search and run commands', group: 'General' },
  { action: 'shortcut-sheet', keys: ['?'], label: 'Show keyboard shortcuts', group: 'General' },
  { action: 'create-task', keys: ['C'], label: 'Create a task', group: 'General' },
  { action: 'search', keys: ['/'], label: 'Search messages and pages', group: 'General' },
  { action: 'go-home', keys: ['G', 'H'], label: 'Home', group: 'Go to' },
  { action: 'go-inbox', keys: ['G', 'I'], label: 'Inbox', group: 'Go to' },
  { action: 'go-messages', keys: ['G', 'M'], label: 'Messages', group: 'Go to' },
  { action: 'go-tasks', keys: ['G', 'T'], label: 'Tasks', group: 'Go to', inProject: true },
  { action: 'go-board', keys: ['G', 'B'], label: 'Board', group: 'Go to', inProject: true },
  { action: 'go-pages', keys: ['G', 'P'], label: 'Pages', group: 'Go to', inProject: true },
  { action: 'go-decisions', keys: ['G', 'D'], label: 'Decisions', group: 'Go to', inProject: true },
  { action: 'go-settings', keys: ['G', 'S'], label: 'Project settings', group: 'Go to', inProject: true },
  { action: 'go-appearance', keys: ['G', 'A'], label: 'Appearance', group: 'Go to' },
]

const SEQUENCE_TIMEOUT_MS = 1_200

type KeyLike = Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'target' | 'isComposing'>

/** Typing in a field, or a menu or dialog that has its own keys. */
export function isTypingTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false
  if (target.isContentEditable) return true
  if (target.closest('input, textarea, select, [contenteditable="true"], [role="textbox"], [role="combobox"]')) return true
  return Boolean(target.closest('[role="dialog"], [role="menu"], [role="listbox"]'))
}

/**
 * Turns key presses into shortcut actions. Single letters only count outside
 * text fields; ⌘K works everywhere. Call `press` for each keydown.
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
    if (event.metaKey || event.ctrlKey || event.altKey) return null
    if (isTypingTarget(event.target)) {
      pending = null
      return null
    }

    if (pending && now() - pending.at <= SEQUENCE_TIMEOUT_MS) {
      const first = pending.key
      pending = null
      const match = SHORTCUTS.find((shortcut) => shortcut.keys.length === 2 && shortcut.keys[0].toLowerCase() === first && shortcut.keys[1].toLowerCase() === key)
      if (match) return match.action
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

/** A task key such as "AWD-12": a Project key, a dash and a number. */
export const TASK_KEY_PATTERN = /^([A-Za-z][A-Za-z0-9]{1,9})-([1-9][0-9]{0,8})$/

export function parseTaskKey(value: string): { projectKey: string; number: number; key: string } | null {
  const match = TASK_KEY_PATTERN.exec(value.trim())
  if (!match) return null
  const projectKey = match[1].toUpperCase()
  const number = Number(match[2])
  return { projectKey, number, key: `${projectKey}-${number}` }
}
