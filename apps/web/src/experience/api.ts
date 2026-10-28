import { useQuery, type QueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { fetchLayout, layoutKeys, normalizeEffective, projectLayoutPath, workspaceLayoutPath, type EffectiveLayout, type LayoutResponse } from './layout'
import type { Layout, SavedPreset } from './registry'

// What the Layout settings pages read and write. The pages share their GETs'
// cache with the hooks every page uses, so a change shows everywhere at once.

export type RuleRole = 'admin' | 'member' | 'viewer'

/** One role in the project, or one person, gets a preset. */
export type LayoutRule = {
  id: string
  role: RuleRole | null
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

/** A write answers with the layout as it now is, in the same shape a read has. */
async function write(path: string, method: 'PUT', json: unknown): Promise<LayoutResponse> {
  const raw = await requestJson<Record<string, unknown>>(path, { method, json })
  return { ...raw, effective: normalizeEffective(raw.effective) }
}

export const saveProjectLayout = (projectId: string, layout: Layout | null, expectedVersion: number) =>
  write(projectLayoutPath(projectId), 'PUT', { layout, expectedVersion })

export const saveProjectRules = (projectId: string, rules: { role?: RuleRole; actorId?: string; preset: string }[]) =>
  write(`${projectLayoutPath(projectId)}/rules`, 'PUT', { rules })

export const saveWorkspaceLayout = (organizationId: string, layout: Layout | null, expectedVersion: number) =>
  write(workspaceLayoutPath(organizationId), 'PUT', { layout, expectedVersion })

export const saveMyView = (organizationId: string, hidden: string[], showHidden: boolean) =>
  write(`${workspaceLayoutPath(organizationId)}/mine`, 'PUT', { hidden, showHidden })

export const createPreset = (organizationId: string, preset: { name: string; description: string; layout: Layout }) =>
  requestJson<SavedPreset>(`${workspaceLayoutPath(organizationId)}/presets`, { method: 'POST', json: preset })

export const changePreset = (
  organizationId: string,
  presetId: string,
  change: { name?: string; description?: string; layout?: Layout; expectedVersion?: number },
) =>
  requestJson<SavedPreset>(`${workspaceLayoutPath(organizationId)}/presets/${encodeURIComponent(presetId)}`, { method: 'PATCH', json: change })

export const removePreset = (organizationId: string, presetId: string) =>
  requestJson<void>(`${workspaceLayoutPath(organizationId)}/presets/${encodeURIComponent(presetId)}`, { method: 'DELETE' })

/** Every layout read so far, in a project or not: presets and "My view" can change them all. */
export function refreshLayouts(queryClient: QueryClient) {
  return queryClient.invalidateQueries({
    predicate: (query) => (query.queryKey[0] === 'project' || query.queryKey[0] === 'organization') && query.queryKey[2] === 'layout',
  })
}

export { layoutKeys }
