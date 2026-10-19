import { useQuery } from '@tanstack/react-query'

import { requestJson } from '../api/client'

/** Where a value is stored. */
export type SettingScope = 'workspace' | 'project' | 'user'
/** Where an effective value came from. */
export type SettingSource = 'launch' | SettingScope | 'default'

export type SettingValueType =
  | { type: 'bool' }
  | { type: 'integer'; min: number; max: number }
  | { type: 'text'; maxChars: number; optional: boolean }
  | { type: 'identifier'; optional: boolean }
  | { type: 'choice'; options: string[] }

export type SettingDefinition = {
  key: string
  label: string
  description: string
  valueType: SettingValueType
  scopes: SettingScope[]
  launchOverride: boolean
  default: unknown
}

export type SettingLayer = {
  scope: SettingScope
  value: unknown
  version: number
  updatedAt: string
  updatedByActorId: string | null
}

export type ResolvedSetting = {
  key: string
  value: unknown
  source: SettingSource
  overrides: SettingSource | null
  fallbackValue: unknown
  /** Every stored value, most specific first. */
  layers: SettingLayer[]
  default: unknown
}

export type EffectiveSettings = {
  organizationId: string
  projectId: string | null
  settings: ResolvedSetting[]
  capabilities: { manageWorkspace: boolean; manageProject: boolean; manageOwn: boolean }
}

/** Which workspace or Project a settings page reads and writes. */
export type SettingsTarget =
  | { scope: 'workspace'; organizationId: string }
  | { scope: 'project'; organizationId: string; projectId: string }

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {}
}

function normalizeValueType(value: unknown): SettingValueType {
  const raw = record(value)
  switch (raw.type) {
    case 'bool':
      return { type: 'bool' }
    case 'integer':
      return { type: 'integer', min: Number(raw.min ?? 0), max: Number(raw.max ?? Number.MAX_SAFE_INTEGER) }
    case 'text':
      // The server sends the field as written in Rust (`max_chars`).
      return { type: 'text', maxChars: Number(raw.maxChars ?? raw.max_chars ?? 200), optional: raw.optional === true }
    case 'choice':
      return { type: 'choice', options: Array.isArray(raw.options) ? raw.options.map(String) : [] }
    default:
      return { type: 'identifier', optional: raw.optional !== false }
  }
}

export function normalizeDefinition(value: unknown): SettingDefinition {
  const raw = record(value)
  return {
    key: String(raw.key ?? ''),
    label: String(raw.label ?? raw.key ?? ''),
    description: String(raw.description ?? ''),
    valueType: normalizeValueType(raw.valueType),
    scopes: Array.isArray(raw.scopes) ? (raw.scopes.filter((scope) => typeof scope === 'string') as SettingScope[]) : [],
    launchOverride: raw.launchOverride === true,
    default: raw.default ?? null,
  }
}

export const settingsKeys = {
  registry: ['settings', 'registry'] as const,
  effective: (target: SettingsTarget) =>
    target.scope === 'project'
      ? (['project', target.projectId, 'settings', 'effective'] as const)
      : (['organization', target.organizationId, 'settings', 'effective'] as const),
}

export async function getSettingsRegistry() {
  const response = await requestJson<{ settings?: unknown[] }>('/api/v1/settings/registry')
  return (response.settings ?? []).map(normalizeDefinition).filter((definition) => definition.key)
}

export function getEffectiveSettings(target: SettingsTarget) {
  const query =
    target.scope === 'project'
      ? `projectId=${encodeURIComponent(target.projectId)}`
      : `organizationId=${encodeURIComponent(target.organizationId)}`
  return requestJson<EffectiveSettings>(`/api/v1/settings/effective?${query}`)
}

function targetBody(target: SettingsTarget) {
  return target.scope === 'project'
    ? { scope: 'project' as const, organizationId: target.organizationId, projectId: target.projectId }
    : { scope: 'workspace' as const, organizationId: target.organizationId }
}

export function setSettingValue(target: SettingsTarget, key: string, value: unknown, expectedVersion: number) {
  return requestJson<{ setting: ResolvedSetting }>(`/api/v1/settings/values/${encodeURIComponent(key)}`, {
    method: 'PUT',
    json: { ...targetBody(target), value, expectedVersion },
  })
}

export function resetSettingValue(target: SettingsTarget, key: string, expectedVersion: number) {
  return requestJson<{ setting: ResolvedSetting }>(`/api/v1/settings/values/${encodeURIComponent(key)}/reset`, {
    method: 'POST',
    json: { ...targetBody(target), expectedVersion },
  })
}

export function useSettingsRegistry(enabled = true) {
  return useQuery({
    queryKey: settingsKeys.registry,
    queryFn: getSettingsRegistry,
    // The registry only changes when the server is upgraded.
    staleTime: Infinity,
    enabled,
  })
}

export function useEffectiveSettings(target: SettingsTarget) {
  return useQuery({
    queryKey: settingsKeys.effective(target),
    queryFn: () => getEffectiveSettings(target),
  })
}

// ---------------------------------------------------------------------------
// Inheritance, as one scope sees it

const PRECEDENCE: Record<SettingScope, number> = { project: 0, user: 1, workspace: 2 }

export type ScopeView = {
  /** The value stored at this scope, if any. */
  own: SettingLayer | null
  /** What applies here without it: the next layer down, or the built-in default. */
  inherited: { source: SettingScope | 'default'; value: unknown }
  /** What this scope shows: its own value, else the inherited one. */
  value: unknown
}

export function viewAtScope(resolved: ResolvedSetting, scope: SettingScope): ScopeView {
  const own = resolved.layers.find((layer) => layer.scope === scope) ?? null
  const below = resolved.layers.filter((layer) => PRECEDENCE[layer.scope] > PRECEDENCE[scope])
  const inherited = below[0]
    ? { source: below[0].scope, value: below[0].value }
    : { source: 'default' as const, value: resolved.default }
  return { own, inherited, value: own ? own.value : inherited.value }
}

/** Where a value is set, as a setting's "Set in" line names it. */
export function scopeName(source: SettingScope | 'default') {
  switch (source) {
    case 'workspace':
      return 'Workspace'
    case 'user':
      return 'Your own settings'
    case 'project':
      return 'This project'
    default:
      return 'Built-in default'
  }
}

function humanizeChoice(value: string) {
  const words = value.replaceAll('_', ' ')
  return words.charAt(0).toUpperCase() + words.slice(1)
}

/** A value as a person reads it: "On", "Fast", "Not set" (or what unset means for this setting). */
export function formatSettingValue(value: unknown, valueType: SettingValueType, options?: SettingOption[], unsetLabel = 'Not set') {
  if (value === null || value === undefined || value === '') return unsetLabel
  if (valueType.type === 'bool') return value === true ? 'On' : 'Off'
  const named = options?.find((option) => option.value === value)
  if (named) return named.label
  if (valueType.type === 'choice' && typeof value === 'string') return humanizeChoice(value)
  return String(value)
}

/**
 * Names for a setting's values: the choices of an identifier setting, such as the project's repositories for
 * `run.repository`, or better words for some of a choice setting's values. Choices with a `group` are listed under it.
 */
export type SettingOption = { value: string; label: string; group?: string }
