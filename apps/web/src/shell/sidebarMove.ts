/**
 * ⌥↑ and ⌥↓ move through the sidebar's rows; with ⇧ they skip to the next
 * unread one, wrapping around. Works from the rendered sidebar, so it follows
 * whatever order and collapsed sections you see.
 */

export type SidebarRow = { current: boolean; unread: boolean }

/** The row to open, or null when there's nowhere to go. */
export function nextSidebarIndex(rows: SidebarRow[], direction: 1 | -1, unreadOnly: boolean): number | null {
  if (rows.length === 0) return null
  const here = rows.findIndex((row) => row.current)
  if (!unreadOnly) {
    if (here === -1) return direction === 1 ? 0 : rows.length - 1
    const next = here + direction
    return next >= 0 && next < rows.length ? next : null
  }
  const start = here === -1 ? (direction === 1 ? -1 : rows.length) : here
  for (let step = 1; step <= rows.length; step += 1) {
    const index = (((start + direction * step) % rows.length) + rows.length) % rows.length
    if (index !== here && rows[index].unread) return index
  }
  return null
}

/** Opens the next row in the sidebar on screen. Returns whether it moved. */
export function moveInSidebar(direction: 1 | -1, unreadOnly: boolean): boolean {
  const links = Array.from(document.querySelectorAll<HTMLAnchorElement>('[data-region="sidebar"] a.aw-item'))
  const index = nextSidebarIndex(
    links.map((link) => ({ current: link.getAttribute('aria-current') === 'page', unread: link.classList.contains('is-unread') })),
    direction,
    unreadOnly,
  )
  if (index === null) return false
  links[index].click()
  return true
}
