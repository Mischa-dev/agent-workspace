import type { PickerField } from '../work-items/FieldPicker'

/**
 * The one list of keys for views (T1.27): the task list and board, a
 * collection's table, and the triage queue. Each view handles its own keys,
 * reading which letter opens which picker from here, and the keyboard
 * shortcut sheet and each view's help list them from here too, so what a
 * view says it does and what it does never drift apart.
 */

export type KeyedView = 'list' | 'board' | 'table' | 'triage'

export type ViewKey = {
  /** The keys as shown, written for a Mac: ["⌘", "Z"]. */
  keys: string[]
  label: string
  /** The views it works in. */
  in: readonly KeyedView[]
}

/** Which letter opens which field's picker on the selected or focused tasks. */
export const PICKER_KEYS: Readonly<Record<string, PickerField>> = { s: 'status', a: 'assignee', l: 'labels', p: 'priority', m: 'milestone' }

/** The triage queue's decisions, by key (T1.23). */
export const TRIAGE_KEYS = { accept: '1', decline: '2', duplicate: '3', snooze: 'h' } as const

const TASKS = ['list', 'board', 'triage'] as const
const LISTS = ['list', 'triage'] as const
const EVERY = ['list', 'board', 'table', 'triage'] as const

export const VIEW_KEYS: readonly ViewKey[] = [
  { keys: ['↓', '↑'], label: 'Next and previous task, opening each in the side panel', in: LISTS },
  { keys: ['J', 'K'], label: 'The same, wherever you are on the page', in: LISTS },
  { keys: ['Home', 'End'], label: 'First and last task', in: LISTS },
  { keys: ['←', '→'], label: 'Next and previous column, opening each card in the side panel', in: ['board'] },
  { keys: ['↓', '↑'], label: 'Next and previous card in a column', in: ['board'] },
  { keys: ['J', 'K'], label: 'Next and previous card, down each column', in: ['board'] },
  { keys: ['Home', 'End'], label: 'First and last card in a column', in: ['board'] },
  { keys: ['↑', '↓', '←', '→'], label: 'Move between cells', in: ['table'] },
  { keys: ['Enter'], label: 'Open the record beside the table, or change the cell', in: ['table'] },
  { keys: ['X'], label: 'Select or unselect', in: TASKS },
  { keys: ['Shift', 'click'], label: 'Select a range', in: TASKS },
  { keys: ['Space'], label: 'Pick up a card, then arrows to move it and Space to drop', in: ['board'] },
  { keys: ['S'], label: 'Change status', in: LISTS },
  { keys: ['A'], label: 'Assign', in: LISTS },
  { keys: ['L'], label: 'Change labels', in: LISTS },
  { keys: ['P'], label: 'Change priority', in: LISTS },
  { keys: ['M'], label: 'Set milestone', in: LISTS },
  { keys: ['S', 'A', 'L', 'P', 'M'], label: 'Status, assignee, labels, priority, milestone', in: ['board'] },
  { keys: ['1'], label: 'Accept the open task', in: ['triage'] },
  { keys: ['2'], label: 'Decline it, with a reason', in: ['triage'] },
  { keys: ['3'], label: 'Mark it a duplicate', in: ['triage'] },
  { keys: ['H'], label: 'Snooze it', in: ['triage'] },
  { keys: ['Enter'], label: 'Open in the side panel', in: TASKS },
  { keys: ['Esc'], label: 'Close the panel, then clear the selection', in: LISTS },
  { keys: ['Esc'], label: 'Put a picked-up card back, close the panel, then clear the selection', in: ['board'] },
  { keys: ['⌘', 'Z'], label: 'Undo your last change', in: EVERY },
  { keys: ['?'], label: 'All keyboard shortcuts', in: EVERY },
]

/** The keys a view of `kind` lists, in order. */
export function viewKeys(kind: KeyedView): ViewKey[] {
  return VIEW_KEYS.filter((key) => key.in.includes(kind))
}

/** What each kind of view is called in the shortcut sheet. */
export const VIEW_NAMES: Record<KeyedView, string> = {
  list: 'Task list',
  board: 'Task board',
  table: 'Collection',
  triage: 'Triage',
}

/** True for ⌘Z (Ctrl+Z elsewhere) without Shift: Undo. */
export function isUndoKey(event: Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey'>) {
  return (event.metaKey || event.ctrlKey) && !event.shiftKey && !event.altKey && event.key.toLowerCase() === 'z'
}
