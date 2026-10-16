import {
  ArrowUpDown,
  Ban,
  Calendar,
  ChevronUp,
  Eye,
  EyeOff,
  GripVertical,
  Hourglass,
  Keyboard,
  Layers,
  ListFilter,
  ListTree,
  Milestone,
  Rows3,
  Tag,
  type LucideIcon,
} from 'lucide-react'

/** Task-only marks the shared icon set doesn't carry. Decorative unless labelled. */
const glyphs = {
  blocked: Ban,
  calendar: Calendar,
  'chevron-up': ChevronUp,
  estimate: Hourglass,
  filter: ListFilter,
  grip: GripVertical,
  group: Layers,
  keyboard: Keyboard,
  lanes: Rows3,
  milestone: Milestone,
  sort: ArrowUpDown,
  subtasks: ListTree,
  tag: Tag,
  unwatch: EyeOff,
  watch: Eye,
} satisfies Record<string, LucideIcon>

export type GlyphName = keyof typeof glyphs

export function TaskGlyph({ label, name, size = 14 }: { label?: string; name: GlyphName; size?: number }) {
  const Glyph = glyphs[name]
  return (
    <Glyph
      aria-hidden={label ? undefined : true}
      aria-label={label}
      className="aw-icon"
      focusable="false"
      role={label ? 'img' : undefined}
      size={size}
      strokeWidth={size <= 14 ? 2.25 : 2}
    />
  )
}
