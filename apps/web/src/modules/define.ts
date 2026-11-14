import type { ComponentType } from 'react'

import type { NounId, PartId } from '../experience/parts'
import type { IconName } from '../ui/icons'

// M1.4: the one registration contract for the web app. Each core module's
// folder has a `module.ts` whose default export is a `WebModule`; the
// registry (`registry.ts`) finds them and the shell, the route table and the
// palette draw from them. A `module.ts` has only type imports: everything it
// shows is loaded when first opened, so registering costs the first page
// load nothing.

/** Where a page belongs: the space the rail highlights, and how it's framed. */
export type RouteSpace = 'home' | 'workspace' | 'project' | 'direct' | 'settings' | 'none'
export type RouteLayout = 'shell' | 'canvas' | 'public' | 'redirect'

/** A page, loaded when first shown. */
export type PageLoader = () => Promise<{ default: ComponentType }>

export type ModuleRoute = {
  /** The address, exactly as the router reads it. */
  path: string
  page: PageLoader
  space: RouteSpace
  /** `shell` unless the page draws its own frame. */
  layout?: Exclude<RouteLayout, 'redirect'>
}

/** A sidebar row's words: fixed, or the layout's word for a noun. */
export type RowLabel = string | { noun: NounId }

/**
 * A row in a sidebar. `space` says which: a project's sidebar (paths after
 * the project's own address) or Home's (paths from the root). A row shows
 * where its module is on and, when it names a part, where the layout shows
 * that part and doesn't show `unless`.
 */
export type SidebarRow = {
  id: string
  space: 'project' | 'home'
  part?: PartId
  unless?: PartId
  icon: IconName
  label: RowLabel
  path: string
  /** The sidebar section that marks this row active (rows holding more than their own page). */
  section?: string
  /** A count beside the row (W1.13 fills in the sources). */
  badge?: 'inbox'
  /** Keeps today's order within its sidebar. */
  rank: number
}

/** A palette entry that opens a page. `rank` keeps today's order within its group. */
export type PaletteEntry = {
  id: string
  space: 'project' | 'workspace'
  label: string
  icon: IconName
  path: string
  rank: number
  shortcut?: string
  keywords?: string[]
}

export type WebModule = {
  /** A manifest id (`modules/<id>.json`). */
  id: string
  /** The shell its pages are drawn in. */
  shell: 'project' | 'home' | 'none'
  routes?: ModuleRoute[]
  /** Pages that need no session. */
  publicRoutes?: ModuleRoute[]
  sidebar?: SidebarRow[]
  palette?: PaletteEntry[]
  /** Shown on a phone's bottom bar (W1.13). */
  phone?: boolean
}
