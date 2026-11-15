import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { normalizeField, type FieldDefinition, type StatusCategory } from './fieldModel'

/**
 * A project's field definitions for one kind of record. Tasks' live at
 * `/projects/{id}/fields`; collections (AWD-90) pass their own path.
 */
export type FieldScope = { projectId: string; path?: string }

function scopePath({ projectId, path }: FieldScope) {
  return path ?? `/api/v1/projects/${encodeURIComponent(projectId)}/fields`
}

export const fieldsKey = (scope: FieldScope, includeArchived = false) =>
  ['project', scope.projectId, 'fields', scope.path ?? 'tasks', includeArchived] as const

export type FieldList = { fields: FieldDefinition[]; canManage: boolean }

export async function listFields(scope: FieldScope, includeArchived = false): Promise<FieldList> {
  const query = includeArchived ? '?includeArchived=true' : ''
  const payload = await requestJson<{ fields?: unknown[]; canManage?: boolean }>(`${scopePath(scope)}${query}`)
  return { fields: (payload.fields ?? []).map(normalizeField), canManage: payload.canManage === true }
}

export type OptionInput = { id?: string; name: string; color?: string; category?: StatusCategory }

export async function createField(
  scope: FieldScope,
  input: { name: string; type: string; description?: string; options?: OptionInput[]; target?: string; idempotencyKey: string },
) {
  const { idempotencyKey, ...json } = input
  return normalizeField(await requestJson<unknown>(scopePath(scope), { method: 'POST', idempotencyKey, json }))
}

export async function updateField(
  scope: FieldScope,
  input: {
    fieldId: string
    expectedVersion: number
    name?: string
    description?: string
    options?: OptionInput[]
    position?: number
    archived?: boolean
    idempotencyKey: string
  },
) {
  const { fieldId, idempotencyKey, ...json } = input
  return normalizeField(
    await requestJson<unknown>(`${scopePath(scope)}/${encodeURIComponent(fieldId)}`, { method: 'PATCH', idempotencyKey, json }),
  )
}

/** The scope's fields: active ones in order, unless archived ones are asked for too. */
export function useFields(scope: FieldScope, includeArchived = false) {
  return useQuery({
    queryKey: fieldsKey(scope, includeArchived),
    queryFn: () => listFields(scope, includeArchived),
    enabled: Boolean(scope.projectId),
    staleTime: 30_000,
  })
}

/** Adding or changing a field refreshes every list of the scope's fields and the records that show them. */
export function useFieldMutations(scope: FieldScope) {
  const client = useQueryClient()
  const refresh = () => client.invalidateQueries({ queryKey: ['project', scope.projectId] })
  return {
    create: useMutation({ mutationFn: (input: Parameters<typeof createField>[1]) => createField(scope, input), onSuccess: refresh }),
    update: useMutation({ mutationFn: (input: Parameters<typeof updateField>[1]) => updateField(scope, input), onSuccess: refresh }),
  }
}
