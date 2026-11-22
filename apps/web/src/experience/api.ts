import { useQuery, type QueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { callOp, ranOrThrow } from '../api/ops'
import { fetchLayout, layoutKeys, normalizeEffective, projectLayoutPath, workspaceLayoutPath, type EffectiveLayout, type LayoutResponse } from './layout'
import type { Layout, SavedPreset } from './registry'

// What the Layout settings pages read and write. The pages share their GETs'
// cache with the hooks every page uses, so a change shows everywhere at once.

export type RuleRole = 'admin' | 'member' | 'viewer'

/** One role in the project, or one person, gets a preset. */
export type LayoutRule = {
  id: string
  role: RuleRole | null
  /** A custom role of the workspace (AWD-74), instead of `role`. */
  roleId?: string | null
  person: { actorId: string; name: string; inProject: boolean } | null
  preset: string
  version: number
}

/** What someone hid for themselves, across the workspace. */
export type MyView = { hidden: string[]; showHidden: boolean; canShowHidden: boolean }

export type LayoutCapabilities = { manage: boolean; manageWorkspace: boolean }

export type WorkspaceLayoutView = {
  organizationId: string
  effective: EffectiveLayout
  layout: Layout | null
  version: number
  savedPresets: SavedPreset[]
  myView: MyView
  capabilities: LayoutCapabilities
}

export type ProjectLayoutView = WorkspaceLayoutView & {
  projectId: string
  workspaceLayout: Layout | null
  rules: LayoutRule[]
}

const NO_VIEW: MyView = { hidden: [], showHidden: false, canShowHidden: false }

function asWorkspaceView(response: LayoutResponse): WorkspaceLayoutView {
  const raw = response as Partial<WorkspaceLayoutView> & LayoutResponse
  return {
    organizationId: raw.organizationId ?? '',
    effective: response.effective,
    layout: raw.layout ?? null,
    version: raw.version ?? 0,
    savedPresets: raw.savedPresets ?? [],
    myView: raw.myView ?? NO_VIEW,
    capabilities: raw.capabilities ?? { manage: false, manageWorkspace: false },
  }
}

function asProjectView(response: LayoutResponse): ProjectLayoutView {
  const raw = response as Partial<ProjectLayoutView> & LayoutResponse
  return { ...asWorkspaceView(response), projectId: raw.projectId ?? '', workspaceLayout: raw.workspaceLayout ?? null, rules: raw.rules ?? [] }
}

export function useProjectLayout(projectId: string) {
  return useQuery({ queryKey: layoutKeys.project(projectId), queryFn: () => fetchLayout(projectLayoutPath(projectId)), select: asProjectView })
}

export function useWorkspaceLayout(organizationId: string) {
  return useQuery({
    queryKey: layoutKeys.workspace(organizationId),
    queryFn: () => fetchLayout(workspaceLayoutPath(organizationId)),
    select: asWorkspaceView,
    enabled: Boolean(organizationId),
  })
}

/**
 * A layout as `layout_update` takes it: the preset, with its parts shown or
 * hidden and its nouns renamed; nothing to follow the next one down.
 */
function layoutInput(layout: Layout | null) {
  if (!layout) return {}
  return {
    preset: layout.preset,
    show: layout.changes && Object.keys(layout.changes).length ? (layout.changes as Record<string, boolean>) : undefined,
    words: layout.words && Object.keys(layout.words).length ? (layout.words as Record<string, { one: string; many: string }>) : undefined,
  }
}

export async function saveProjectLayout(projectId: string, layout: Layout | null, expectedVersion: number) {
  ranOrThrow(await callOp('layout_update', { projectId, ...layoutInput(layout) }, { expectedVersion }), 'Changing the layout')
  return fetchLayout(projectLayoutPath(projectId))
}

export async function saveProjectRules(projectId: string, rules: { role?: RuleRole; roleId?: string; actorId?: string; preset: string }[]) {
  const wanted = rules.map((rule) => ({ role: rule.roleId ?? rule.role, person: rule.actorId, preset: rule.preset }))
  ranOrThrow(await callOp('layout_update', { projectId, rules: wanted }), 'Changing who gets which layout')
  return fetchLayout(projectLayoutPath(projectId))
}

export async function saveWorkspaceLayout(organizationId: string, layout: Layout | null, expectedVersion: number) {
  const answer = await callOp(
    'layout_update',
    { projectId: 'workspace', wholeWorkspace: true, ...layoutInput(layout) },
    { workspaceId: organizationId, expectedVersion },
  )
  ranOrThrow(answer, 'Changing the default layout')
  return fetchLayout(workspaceLayoutPath(organizationId))
}

export async function saveMyView(organizationId: string, hidden: string[], showHidden: boolean): Promise<LayoutResponse> {
  const raw = await requestJson<Record<string, unknown>>(`${workspaceLayoutPath(organizationId)}/mine`, { method: 'PUT', json: { hidden, showHidden } })
  return { ...raw, effective: normalizeEffective(raw.effective) }
}

/** A layout saved as a workspace preset, through `layout_preset_create`. */
export async function createPreset(organizationId: string, preset: { name: string; description: string; layout: Layout }) {
  const { preset: base, show, words } = layoutInput(preset.layout)
  const answer = await callOp(
    'layout_preset_create',
    { projectId: 'workspace', name: preset.name, description: preset.description, base, show, words },
    { workspaceId: organizationId },
  )
  return ranOrThrow(answer, 'Saving the preset')
}

/** A preset renamed, described or reshaped (from a whole layout), through `layout_preset_update`. */
export async function changePreset(
  organizationId: string,
  presetId: string,
  change: { name?: string; description?: string; layout?: Layout; expectedVersion?: number },
) {
  // A whole layout starts the preset over from its base.
  const { preset: base, show, words } = change.layout ? layoutInput(change.layout) : {}
  const answer = await callOp(
    'layout_preset_update',
    { projectId: 'workspace', preset: presetId, name: change.name, description: change.description, base, show, words },
    { workspaceId: organizationId, expectedVersion: change.expectedVersion },
  )
  return ranOrThrow(answer, 'Changing the preset')
}

export async function removePreset(organizationId: string, presetId: string) {
  const answer = await callOp('layout_preset_update', { projectId: 'workspace', preset: presetId, remove: true }, { workspaceId: organizationId })
  return ranOrThrow(answer, 'Removing the preset')
}

/** Every layout read so far, in a project or not: presets and "My view" can change them all. */
export function refreshLayouts(queryClient: QueryClient) {
  return queryClient.invalidateQueries({
    predicate: (query) => (query.queryKey[0] === 'project' || query.queryKey[0] === 'organization') && query.queryKey[2] === 'layout',
  })
}

export { layoutKeys }
