// Pages inside pages (C1.38): a project's pages in tree order, each with the
// titles of the pages it sits in, and the pages a page may go inside.
import type { PageSummary } from './api'

export type TreePage = PageSummary & { depth: number; path: string[] }

/**
 * The pages in tree order: each parent before its children, siblings by
 * position then title. A page whose parent isn't in the list (archived, or
 * not shown) sits at the top.
 */
export function pageTree(pages: PageSummary[]): TreePage[] {
  const ids = new Set(pages.map((page) => page.id))
  const children = new Map<string | null, PageSummary[]>()
  for (const page of pages) {
    const parent = page.parentPageId && ids.has(page.parentPageId) ? page.parentPageId : null
    children.set(parent, [...(children.get(parent) ?? []), page])
  }
  const order = (a: PageSummary, b: PageSummary) => a.position - b.position || a.title.localeCompare(b.title)
  const out: TreePage[] = []
  const seen = new Set<string>()
  const walk = (parent: string | null, path: string[]) => {
    for (const page of [...(children.get(parent) ?? [])].sort(order)) {
      if (seen.has(page.id)) continue
      seen.add(page.id)
      out.push({ ...page, depth: path.length, path })
      walk(page.id, [...path, page.title])
    }
  }
  walk(null, [])
  return out
}

/** The pages `pageId` may go inside: every page but itself and the pages inside it. */
export function parentChoices(pages: PageSummary[], pageId: string): TreePage[] {
  const tree = pageTree(pages)
  const inside = new Set([pageId])
  for (const page of tree) if (page.parentPageId && inside.has(page.parentPageId)) inside.add(page.id)
  return tree.filter((page) => !inside.has(page.id))
}
