import type { IconName } from '../ui'

// M1.7: the icons a built module and its sidebar entries can wear, shared by
// the builder and the sidebar.

/** The icons a module can wear: the shared set's ones that read as a place. */
export const MODULE_ICONS: { value: IconName; label: string }[] = [
  { value: 'table', label: 'Table' },
  { value: 'list', label: 'List' },
  { value: 'board', label: 'Board' },
  { value: 'checklist', label: 'Checklist' },
  { value: 'chart', label: 'Chart' },
  { value: 'inbox', label: 'Inbox' },
  { value: 'bookmark', label: 'Bookmark' },
  { value: 'star', label: 'Star' },
  { value: 'activity', label: 'Activity' },
  { value: 'users', label: 'People' },
  { value: 'repo', label: 'Code' },
  { value: 'whiteboard', label: 'Whiteboard' },
]

export function moduleIcon(icon: string | null | undefined): IconName {
  return MODULE_ICONS.find((choice) => choice.value === icon)?.value ?? 'table'
}
