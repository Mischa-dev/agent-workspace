/**
 * The one "focus belongs elsewhere" rule. A key pressed in a field, menu,
 * listbox, dialog or popover is that control's, not the page's: a list
 * doesn't move and a side panel doesn't close. Shared by `useListKeys`,
 * `SidePanel`, `Sheet` and the grid keys.
 */
export function inOwnControl(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false
  if (target.isContentEditable) return true
  return Boolean(target.closest('input, textarea, select, [role="menu"], [role="listbox"], .aw-popover'))
}

/** A dialog or popover is open above the page: it has the keys. */
export function overlayOpen() {
  return Boolean(document.querySelector('.aw-dialog-layer, .aw-popover'))
}

/** For page-wide keys (a list's j and k): a side panel and dialogs keep theirs too. */
export function belongsElsewhere(target: EventTarget | null) {
  if (inOwnControl(target)) return true
  return target instanceof HTMLElement && Boolean(target.closest('[role="dialog"], #context-region, .aw-k-sidepanel'))
}
