import type { ShortcutAction } from './shortcuts'

/** What the shortcut sheet lists. The keys themselves are handled in shortcuts.ts. */
export type Shortcut = {
  action: ShortcutAction
  /** The keys as shown: ["⌘", "K"], or ["G", "I"] for a sequence. */
  keys: string[]
  label: string
  group: 'General' | 'Conversations' | 'Go to'
  /** Needs a project open. */
  inProject?: boolean
}

export const SHORTCUTS: Shortcut[] = [
  { action: 'command-menu', keys: ['⌘', 'K'], label: 'Search and run commands', group: 'General' },
  { action: 'shortcut-sheet', keys: ['?'], label: 'Show keyboard shortcuts', group: 'General' },
  { action: 'create-task', keys: ['C'], label: 'New task', group: 'General' },
  { action: 'search', keys: ['/'], label: 'Search messages and pages', group: 'General' },
  { action: 'previous-conversation', keys: ['⌥', '↑'], label: 'Previous item in the sidebar', group: 'Conversations' },
  { action: 'next-conversation', keys: ['⌥', '↓'], label: 'Next item in the sidebar', group: 'Conversations' },
  { action: 'previous-unread', keys: ['⌥', '⇧', '↑'], label: 'Previous unread conversation', group: 'Conversations' },
  { action: 'next-unread', keys: ['⌥', '⇧', '↓'], label: 'Next unread conversation', group: 'Conversations' },
  { action: 'mark-all-read', keys: ['⇧', 'Esc'], label: 'Mark all messages read', group: 'Conversations' },
  { action: 'go-home', keys: ['G', 'H'], label: 'Home', group: 'Go to' },
  { action: 'go-inbox', keys: ['G', 'I'], label: 'Inbox', group: 'Go to' },
  { action: 'go-my-work', keys: ['G', 'W'], label: 'My work', group: 'Go to' },
  { action: 'go-later', keys: ['G', 'L'], label: 'Later', group: 'Go to' },
  { action: 'go-messages', keys: ['G', 'M'], label: 'Messages', group: 'Go to' },
  { action: 'go-tasks', keys: ['G', 'T'], label: 'Tasks', group: 'Go to', inProject: true },
  { action: 'go-board', keys: ['G', 'B'], label: 'Board', group: 'Go to', inProject: true },
  { action: 'go-pages', keys: ['G', 'P'], label: 'Pages', group: 'Go to', inProject: true },
  { action: 'go-decisions', keys: ['G', 'D'], label: 'Decisions', group: 'Go to', inProject: true },
  { action: 'go-settings', keys: ['G', 'S'], label: 'Project settings', group: 'Go to', inProject: true },
  { action: 'go-appearance', keys: ['G', 'A'], label: 'Appearance', group: 'Go to' },
]
