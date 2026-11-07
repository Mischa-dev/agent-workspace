import { useMemo } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { queryKeys } from '../api/queryKeys'
import type { SessionResponse } from '../auth/session'
import type { NounId, PartId } from './parts'

// Layouts (AWD-66): which parts of the app each person sees, worked out by
// the server for a project or the workspace. This file loads with every page,
// so it holds only the reading side; the registry and the settings screens
// load with their own pages.

export type Word = { one: string; many: string }

export type LayoutSource = 'person' | 'role' | 'project' | 'workspace' | 'default'

/** Which preset a layout uses. */
export type PresetRef = {
  /** A built-in preset's id, such as `personal_life`, or a saved preset's ID. */
  id: string
  name: string
  builtIn: boolean
  /** The built-in preset it starts from: itself, for a built-in one. */
  base: string
  removed: boolean
}

/** What one person sees, as the server worked it out. */
export type EffectiveLayout = {
  source: LayoutSource
  preset: PresetRef | null
  /** Parts this person doesn't see. */
  hidden: string[]
  /** Parts the layout hides, before "My view" and "Show hidden parts". */
  layoutHidden: string[]
  /** Nouns the layout renames. */
  words: Partial<Record<NounId, Word>>
  /** Parts to put first. */
  lead: string[]
  /** An owner or admin is seeing what the layout hides. */
  showingHidden: boolean
  /** Modules that are off for this person (AWD-75); their parts are in `hidden` too. */
  modulesOff: string[]
}

/** A layout GET: what the person sees, and what the Layout settings pages read. */
export type LayoutResponse = { effective: EffectiveLayout } & Record<string, unknown>

/** Each noun's own words. */
export const OWN_WORDS: Record<NounId, Word> = {
  task: { one: 'task', many: 'tasks' },
  milestone: { one: 'milestone', many: 'milestones' },
  page: { one: 'page', many: 'pages' },
  channel: { one: 'channel', many: 'channels' },
}

const SOURCES: readonly LayoutSource[] = ['person', 'role', 'project', 'workspace', 'default']

const record = (value: unknown): Record<string, unknown> =>
  value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {}

const strings = (value: unknown) => (Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [])

/** Read a layout defensively: anything missing or odd means nothing is hidden. */
export function normalizeEffective(value: unknown): EffectiveLayout {
  const raw = record(value)
  const rawWords = record(raw.words)
  const words: Partial<Record<NounId, Word>> = {}
  for (const noun of Object.keys(OWN_WORDS) as NounId[]) {
    const word = record(rawWords[noun])
    if (typeof word.one === 'string' && typeof word.many === 'string' && word.one && word.many) {
      words[noun] = { one: word.one, many: word.many }
    }
  }
  const preset = record(raw.preset)
  return {
    source: SOURCES.find((source) => source === raw.source) ?? 'default',
    preset:
      typeof preset.id === 'string'
        ? {
            id: preset.id,
            name: typeof preset.name === 'string' ? preset.name : preset.id,
            builtIn: preset.builtIn === true,
            base: typeof preset.base === 'string' ? preset.base : preset.id,
            removed: preset.removed === true,
          }
        : null,
    hidden: strings(raw.hidden),
    layoutHidden: strings(raw.layoutHidden),
    words,
    lead: strings(raw.lead),
    showingHidden: raw.showingHidden === true,
    modulesOff: strings(raw.modulesOff),
  }
}

/** What the person sees, with helpers for pages to ask. */
export type Experience = {
  /** Whether the person sees this part. Everything shows while the layout loads, or if it can't. */
  shows: (part: PartId) => boolean
  /** Whether a module (`tasks`, `pages`, ...) is on for the person. All are while the layout loads. */
  moduleOn: (module: string) => boolean
  /**
   * Whether `moduleOn` reflects the person's module settings (AWD-75):
   * `loading` until the layout answers (a layout remembered on this device
   * doesn't count), `known` once it has, and `unknown` where there's none to
   * ask for or it couldn't load, when every module counts as on, as before.
   * Gating in the app waits while it's `loading`; it hides, it doesn't secure.
   */
  modules: ModuleSettings
  /** A noun as the layout says it, in lower case: "task", or "to-do" in Personal life. */
  word: (noun: NounId, form?: 'one' | 'many') => string
  /** The same, with a capital first letter, for labels and titles: "Tasks", "To-dos". */
  label: (noun: NounId, form?: 'one' | 'many') => string
  /** Parts to put first, such as Runs in Agent lab. */
  lead: readonly string[]
  effective: EffectiveLayout | null
}

export type ModuleSettings = 'loading' | 'known' | 'unknown'

/** "to-dos" to "To-dos". */
export function capitalized(text: string) {
  return text ? text.charAt(0).toLocaleUpperCase() + text.slice(1) : text
}

export function experienceFrom(
  effective: EffectiveLayout | null | undefined,
  modules: ModuleSettings = 'known',
): Experience {
  const hidden = new Set(effective?.hidden ?? [])
  const off = new Set(effective?.modulesOff ?? [])
  const word = (noun: NounId, form: 'one' | 'many' = 'one') => (effective?.words[noun] ?? OWN_WORDS[noun])[form]
  return {
    shows: (part) => !hidden.has(part),
    moduleOn: (module) => !off.has(module),
    modules,
    word,
    label: (noun, form = 'one') => capitalized(word(noun, form)),
    lead: effective?.lead ?? [],
    effective: effective ?? null,
  }
}

export const layoutKeys = {
  project: (projectId: string) => ['project', projectId, 'layout'] as const,
  workspace: (organizationId: string) => ['organization', organizationId, 'layout'] as const,
}

export const projectLayoutPath = (projectId: string) => `/api/v1/projects/${encodeURIComponent(projectId)}/layout`
export const workspaceLayoutPath = (organizationId: string) => `/api/v1/organizations/${encodeURIComponent(organizationId)}/layout`

export async function fetchLayout(path: string): Promise<LayoutResponse> {
  const raw = record(await requestJson<unknown>(path))
  return { ...raw, effective: normalizeEffective(raw.effective) }
}

// The last layout seen on this device, per person and place, so a page that
// opens again doesn't show every part for a moment before its layout loads.
const CACHE_PREFIX = 'aw-layout:'

function remembered(key: string | null): LayoutResponse | undefined {
  if (!key) return undefined
  try {
    const text = window.localStorage.getItem(CACHE_PREFIX + key)
    return text ? { effective: normalizeEffective(JSON.parse(text)) } : undefined
  } catch {
    return undefined
  }
}

function remember(key: string | null, effective: EffectiveLayout) {
  if (!key) return
  try {
    window.localStorage.setItem(CACHE_PREFIX + key, JSON.stringify(effective))
  } catch {
    // Storage can be full or blocked; the layout still applies once loaded.
  }
}

/** The signed-in session, read from the cache without asking for it. */
function useCachedSession() {
  return useQueryClient().getQueryData<SessionResponse>(queryKeys.session)
}

function useExperience(queryKey: readonly unknown[], path: string, id: string, scope: string): Experience {
  const actorId = useCachedSession()?.user?.actorId
  const cacheKey = id && actorId ? `${actorId}:${scope}:${id}` : null
  const query = useQuery({
    queryKey,
    queryFn: async () => {
      const layout = await fetchLayout(path)
      remember(cacheKey, layout.effective)
      return layout
    },
    enabled: Boolean(id),
    staleTime: 60_000,
    placeholderData: () => remembered(cacheKey),
  })
  const effective = query.data?.effective
  // Known once the server has answered (a later failed refetch keeps that
  // answer); unknown after a failure with no answer, also while it's asked
  // again, so a page doesn't flip back to waiting each time it mounts.
  const modules: ModuleSettings = !id
    ? 'unknown'
    : query.dataUpdatedAt > 0 && !query.isPlaceholderData
      ? 'known'
      : query.errorUpdatedAt > 0
        ? 'unknown'
        : 'loading'
  return useMemo(() => experienceFrom(effective, modules), [effective, modules])
}

/** What the person sees in a project. */
export function useProjectExperience(projectId: string | null | undefined): Experience {
  const id = projectId ?? ''
  return useExperience(layoutKeys.project(id), projectLayoutPath(id), id, 'project')
}

/** What the person sees on workspace pages, such as Home. */
export function useWorkspaceExperience(organizationId: string | null | undefined): Experience {
  const id = organizationId ?? ''
  return useExperience(layoutKeys.workspace(id), workspaceLayoutPath(id), id, 'workspace')
}

/** What the person sees on Home and the pages beside it. */
export function useHomeExperience(): Experience {
  return useWorkspaceExperience(useHomeWorkspaceId())
}

/** The workspace Home and the pages beside it follow: the one the person chose, else their first. */
export function useHomeWorkspaceId(): string | null {
  const session = useCachedSession()
  if (!session?.user) return null
  return (
    session.organizations.find((organization) => organization.id === session.user?.defaultOrganizationId)?.id ??
    session.organizations[0]?.id ??
    null
  )
}
