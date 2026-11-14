/**
 * The kinds of view (T1.2): the one list tasks, collections and module
 * manifests name kinds from. The server's `crate::views::ViewKind` is the
 * same list; `modules/contract.ts` re-exports it.
 */
export type ViewKind = 'table' | 'board' | 'list' | 'calendar' | 'timeline' | 'gallery' | 'chart'

export const VIEW_KINDS: readonly ViewKind[] = ['table', 'board', 'list', 'calendar', 'timeline', 'gallery', 'chart']
