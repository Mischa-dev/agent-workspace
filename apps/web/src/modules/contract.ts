// The shared contract (AWD-75, principle 1 of the build-out plan), as the web
// app reads it. It mirrors apps/server/src/modules/contract.rs; see
// docs/plans/modules.md.

/** What a tool does, in the one pattern every module's tools follow. */
export type ToolVerb = 'describe' | 'query' | 'create' | 'change' | 'suggest'

/** How one thing relates to another; every link reads both ways. */
export type LinkKind = 'part_of' | 'blocks' | 'about' | 'made_from' | 'relates_to' | 'duplicates'

/** A field's type, shared by tasks, collections and every module's records, named as task fields (AWD-89) store them. */
export type FieldType =
  | 'text'
  | 'long_text'
  | 'number'
  | 'date'
  | 'checkbox'
  | 'url'
  | 'single_choice'
  | 'multiple_choice'
  | 'status'
  | 'person'
  | 'people'
  | 'labels'
  | 'link'

/** A way to look at many things at once. */
export type ViewType = 'list' | 'table' | 'board' | 'calendar' | 'timeline' | 'gallery'

/** A piece of screen the app draws for a module. */
export type PieceType = 'list' | 'table' | 'board' | 'detail' | 'fields' | 'form' | 'card' | 'actions'

export type Piece = {
  type: PieceType
  /** The kind of thing it shows. */
  thing: string
  /** Fields in order: the shared ones (`title`, `status`, `owner`) or the thing's own. */
  fields?: string[]
  groupBy?: string
  actions?: string[]
}

/** How a value reads as a chip; as the app's StatusChip draws them. */
export type ChipTone = 'muted' | 'agent' | 'warning' | 'success' | 'danger'

export type FieldDecl = {
  id: string
  label: string
  type: FieldType
  options?: string[]
  /** How a label or choice reads as a chip, by value; the rest read muted. */
  tones?: Record<string, ChipTone>
}

export type ThingDecl = { kind: string; one: string; many: string; href: string; fields: FieldDecl[] }

export type EmbedDecl = { id: string; label: string; description: string; thing: string; piece: Piece }

/** One thing, as any module, tool or screen sees it. */
export type Thing = {
  id: string
  kind: string
  module: string
  title: string
  status?: string
  owner?: { id: string; name: string }
  projectId?: string
  href: string
  /** Values of the thing's own fields, by field id. */
  fields?: Record<string, FieldValue>
}

export type FieldValue = string | number | boolean | string[] | null

export type ThingLink = {
  kind: LinkKind
  direction: 'outgoing' | 'incoming'
  /** The link in words from this side, such as "Blocked by". */
  label: string
  other: Thing
}

/** The fields every thing has, whatever its kind. */
export const SHARED_FIELDS: FieldDecl[] = [
  { id: 'title', label: 'Title', type: 'text' },
  { id: 'status', label: 'Status', type: 'status' },
  { id: 'owner', label: 'Owner', type: 'person' },
]
