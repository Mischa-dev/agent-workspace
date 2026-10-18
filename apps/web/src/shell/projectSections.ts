/** Pages that sit under Work items in a Project's sidebar, shown as tabs on each. */
const workPages = ['work-items', 'milestones', 'timeline', 'next', 'suggestions', 'graph', 'plan-import']
/** Pages that sit under Pages. */
const memoryPages = ['pages', 'decisions']

/** Which sidebar row holds the page at this path, for rows that hold more than their own page. */
export function projectSection(pathname: string, base: string): 'work' | 'pages' | null {
  if (!pathname.startsWith(`${base}/`)) return null
  const page = pathname.slice(base.length + 1).split('/')[0]
  if (workPages.includes(page)) return 'work'
  if (memoryPages.includes(page)) return 'pages'
  return null
}
