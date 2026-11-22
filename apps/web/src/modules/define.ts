import type { QueryClient } from '@tanstack/react-query'
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

/** What an embed's path says: named values read from it, such as a sheet's id. */
export type EmbedParams = Readonly<Record<string, string | null>>

/** What an embed's view is given: what its path says, the address and the label the page keeps. */
export type EmbedViewProps = { params: EmbedParams; label: string; href: string }

/** What a picker that makes an embed is given (the page editor's toolbar). */
export type EmbedPickerProps = {
  /** The page's space, whose things it offers. */
  projectId: string
  onPick: (embed: { href: string; label: string }) => void
  onClose: () => void
}

/**
 * Something of a module's drawn live inside a page (M1.5): an embed block
 * whose path it reads. The page draws the first registration whose `match`
 * reads the path; a path none reads is drawn as the thing its module's
 * manifest declares, or a link. The view, and the picker the page editor's
 * toolbar offers to make one, load when first used.
 */
export type EmbedRegistration = {
  id: string
  /** The path's named values when it's one of this module's, else null. Pure: no imports. */
  match: (path: string, search: URLSearchParams) => EmbedParams | null
  view: () => Promise<{ default: ComponentType<EmbedViewProps> }>
  /** What it is while it loads, such as "the collection". */
  loading: string
  /** What a pasted address of one is called when the path names no single thing, such as "Tasks". */
  label?: string
  /** The toolbar's way to make one, where the layout shows `part`. */
  picker?: {
    label: string
    part?: PartId
    view: () => Promise<{ default: ComponentType<EmbedPickerProps> }>
  }
}

/**
 * Keeps open pages current when something of the module's changes (M1.5):
 * the live-update events it answers, by type, and its handler, loaded the
 * first time one arrives. The handler refreshes exactly what the event names.
 */
export type LiveRegistration = {
  events: readonly string[]
  handler: () => Promise<{ default: LiveHandler }>
}

/** Refreshes what one event names; `queryClient` is TanStack Query's. */
export type LiveHandler = (queryClient: QueryClient, event: { type: string; projectId: string; payload: Record<string, unknown> }) => void

/**
 * A tab on a thing's detail view (M1.5): shown on things of `thing`'s kind
 * where the layout shows `part`, loaded when opened.
 */
export type ThingTabRegistration = {
  id: string
  thing: string
  label: string
  icon: IconName
  part?: PartId
  rank: number
  view: () => Promise<{ default: ComponentType<{ thingId: string; projectId?: string }> }>
}

/**
 * Something the message composer offers beside sending (M1.5): an extra in
 * its menu, where the layout shows `part`, loaded when picked.
 */
export type ComposerExtraRegistration = {
  id: string
  label: string
  icon: IconName
  part?: PartId
  rank: number
  view: () => Promise<{ default: ComponentType<{ conversationId: string; onDone: (insert?: string) => void }> }>
}

/**
 * Something "New" makes (M1.5): an entry in the space's make menu that opens
 * where the new thing is started.
 */
export type MakerRegistration = {
  id: string
  label: string
  icon: IconName
  /** After the space's address. */
  path: string
  part?: PartId
  rank: number
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
  /** What it draws inside pages (M1.5). */
  embeds?: EmbedRegistration[]
  /** The live-update events it answers (M1.5). */
  live?: LiveRegistration[]
  /** Tabs it adds to things' detail views (M1.5). */
  tabs?: ThingTabRegistration[]
  /** What it adds to the message composer (M1.5). */
  composer?: ComposerExtraRegistration[]
  /** What it adds to a space's make menu (M1.5). */
  makers?: MakerRegistration[]
}
