import type { WebModule } from '../modules/define'
import { moduleShortcuts, WEB_MODULES } from '../modules/registry'
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

/** The shell's own keys, before the modules' "Go to" keys. */
const SHELL: Shortcut[] = [
  { action: 'command-menu', keys: ['⌘', 'K'], label: 'Search and run commands', group: 'General' },
  { action: 'shortcut-sheet', keys: ['?'], label: 'Show keyboard shortcuts', group: 'General' },
  { action: 'create-task', keys: ['C'], label: 'New task', group: 'General' },
  { action: 'search', keys: ['/'], label: 'Search messages and pages', group: 'General' },
  { action: 'previous-conversation', keys: ['⌥', '↑'], label: 'Previous item in the sidebar', group: 'Conversations' },
  { action: 'next-conversation', keys: ['⌥', '↓'], label: 'Next item in the sidebar', group: 'Conversations' },
  { action: 'previous-unread', keys: ['⌥', '⇧', '↑'], label: 'Previous unread conversation', group: 'Conversations' },
  { action: 'next-unread', keys: ['⌥', '⇧', '↓'], label: 'Next unread conversation', group: 'Conversations' },
  { action: 'mark-read', keys: ['Esc'], label: 'Mark this conversation read', group: 'Conversations' },
  { action: 'mark-all-read', keys: ['⇧', 'Esc'], label: 'Mark all messages read', group: 'Conversations' },
  { action: 'all-unreads', keys: ['⌘', '⇧', 'A'], label: 'All unread', group: 'Conversations' },
  { action: 'all-threads', keys: ['⌘', '⇧', 'T'], label: 'Threads', group: 'Conversations' },
  { action: 'go-home', keys: ['G', 'H'], label: 'Home', group: 'Go to' },
]

/** The shell's settings keys, after the modules'. */
const SHELL_LAST: Shortcut[] = [
  { action: 'go-settings', keys: ['G', 'S'], label: 'Project settings', group: 'Go to', inProject: true },
  { action: 'go-appearance', keys: ['G', 'A'], label: 'Appearance', group: 'Go to' },
]

/** The sheet: the shell's keys and every registered module's (W1.13). */
export function shortcutSheet(modules: readonly WebModule[] = WEB_MODULES): Shortcut[] {
  return [
    ...SHELL,
    ...moduleShortcuts(modules).map(
      (shortcut): Shortcut => ({
        action: shortcut.action,
        keys: ['G', shortcut.key.toUpperCase()],
        label: shortcut.label,
        group: 'Go to',
        ...(shortcut.inProject ? { inProject: true } : {}),
      }),
    ),
    ...SHELL_LAST,
  ]
}

export const SHORTCUTS: Shortcut[] = shortcutSheet()
