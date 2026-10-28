import {
  Bookmark,
  Bot,
  Bug,
  CalendarDays,
  CodeXml,
  Columns3,
  Eye,
  FlaskConical,
  GraduationCap,
  Heart,
  House,
  LifeBuoy,
  ListChecks,
  Map as MapIcon,
  Megaphone,
  Minus,
  PenLine,
  Rocket,
  Sparkles,
} from 'lucide-react'

import type { IconGlyph } from '../ui'
import { REGISTRY, everything, presetShape, type SavedPreset, type Shape } from './registry'

const PRESET_ICONS: Record<string, IconGlyph> = {
  software_team: CodeXml,
  startup: Rocket,
  personal_life: Heart,
  household: House,
  bug_reporting: Bug,
  simple_todo: ListChecks,
  kanban: Columns3,
  writing: PenLine,
  research: FlaskConical,
  school: GraduationCap,
  event_planning: CalendarDays,
  content_calendar: Megaphone,
  support_desk: LifeBuoy,
  agent_lab: Bot,
  planning_only: MapIcon,
  stakeholder: Eye,
  minimal: Minus,
}

/** One card: a built-in preset, a saved one, or `id: null` for "whatever applies otherwise". */
export type PresetChoice = { id: string | null; name: string; description: string; icon: IconGlyph; shape: Shape; tag?: string }

/** The cards to pick from: what applies without a choice, the built-in presets, then the workspace's own. */
export function presetChoices(saved: SavedPreset[], fallback: Omit<PresetChoice, 'id' | 'icon'> & { icon?: IconGlyph }, current?: string | null): PresetChoice[] {
  return [
    { ...fallback, id: null, icon: fallback.icon ?? Sparkles },
    ...REGISTRY.presets.map((preset) => ({
      id: preset.id,
      name: preset.name,
      description: preset.description,
      icon: PRESET_ICONS[preset.id] ?? Sparkles,
      shape: presetShape(preset.id, saved) ?? everything(),
    })),
    ...saved
      .filter((preset) => !preset.removed || preset.id === current)
      .map((preset) => ({
        id: preset.id,
        name: preset.name,
        description: preset.description || 'Saved in this workspace.',
        icon: Bookmark,
        shape: presetShape(preset.id, saved) ?? everything(),
        tag: preset.removed ? 'Removed' : 'Saved',
      })),
  ]
}
