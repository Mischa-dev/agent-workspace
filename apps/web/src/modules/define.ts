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

/**
 * A destination on a phone's bottom bar (W1.29): a Home page (`home`), or a
 * page of the space you're in, else the one you opened last (`project`).
 */
export type PhoneTab = {
  id: string
  space: 'project' | 'home'
  part?: PartId
  icon: IconName
  label: RowLabel
  path: string
  badge?: 'inbox'
  /** The bar's order, Home first. */
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

/**
 * A keyboard shortcut a module brings (W1.13): G then a letter, opening one
 * of its pages. The shell's matcher, its handlers and the shortcut sheet are
 * drawn from these, so a module's keys come and go with it.
 */
export type ShortcutRegistration = {
  /** The action's name, `go-<page>`; the module's manifest claims it. */
  action: `go-${string}`
  /** The letter after G, lower case. */
  key: string
  /** What the sheet calls it. */
  label: string
  /** Where it goes: from the root, or, with `inProject`, after the open project's address. */
  path: string
  /** Acts in the open project (and follows that project's modules). */
  inProject?: boolean
  /** Keeps today's order on the sheet. */
  rank: number
}

/**
 * A kind of thing the module opens (W1.13), for the shell's "open anything"
 * and for drawing a link to one: its label and icon. K3's thing registry
 * names the kinds; this is how the web app shows them.
 */
export type ThingRegistration = {
  kind: string
  label: string
  icon: IconName
}

/**
 * Something the module lets a person capture quickly (W1.13; W1.32 shows
 * them on the capture page and share-in): a page that starts it.
 */
export type CaptureRegistration = {
  id: string
  label: string
  icon: IconName
  /** From the root, or, with `inProject`, after a project's address. */
  path: string
  inProject?: boolean
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
  /** Its place on a phone's bottom bar (W1.29). */
  phone?: PhoneTab
  /** G-then-a-letter keys to its pages (W1.13). */
  shortcuts?: ShortcutRegistration[]
  /** The kinds of thing it opens, as the shell shows them (W1.13). */
  things?: ThingRegistration[]
  /** What it lets a person capture quickly (W1.13). */
  capture?: CaptureRegistration[]
}
