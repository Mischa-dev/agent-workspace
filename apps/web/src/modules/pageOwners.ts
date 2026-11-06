import { matchRoutes } from 'react-router-dom'
import pages from 'virtual:modules/slots/pages'

// AWD-75: the module each page belongs to, matched the way the router matches
// routes (static before dynamic, the catch-all last).

const PAGE_ROUTES = pages.map(([path, module]) => ({ path, module }))

/** The module the page at `pathname` belongs to, or null. */
export function moduleOfPath(pathname: string): string | null {
  const matches = matchRoutes(PAGE_ROUTES, pathname)
  const route = matches?.[matches.length - 1]?.route as { module?: string } | undefined
  return route?.module ?? null
}
