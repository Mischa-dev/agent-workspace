import type { QueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { refreshLayouts } from '../experience/api'
import { onboardingKeys } from '../onboarding/api'
import { roleKeys } from '../settings/workspace/roles'

// AWD-76: workspace templates. A template sets the modules, the layout
// everyone starts with, roles and the role new members get, and can bring a
// starter project with channels and collections. Built-in ones come first,
// Everything first; a workspace can save itself as one too.

export type TemplateRole = {
  name: string
  description?: string
  baseRole?: 'admin' | 'member' | 'viewer'
  permissions?: string[]
  agentLimit?: string[]
  modulesOff?: string[]
}

export type TemplateDefinition = {
  modules?: string[]
  layout?: string
  roles?: TemplateRole[]
  defaultRole?: string
  project?: { name: string; key: string; channels?: string[]; collections?: { name: string }[] }
}

export type WorkspaceTemplate = {
  id: string
  name: string
  description: string
  builtIn: boolean
  /** What using it does, in plain words, one line each. */
  whatItDoes: string[]
  definition: TemplateDefinition
  savedBy?: string
  savedAt?: string
}

export type TemplateUse = {
  template: string
  templateName: string
  projectId: string | null
  summary: string[]
  usedBy: string
  usedAt: string
}

export type TemplatesResponse = {
  organizationId: string
  templates: WorkspaceTemplate[]
  lastUsed?: TemplateUse
  canUse: boolean
}

export type Applied = {
  template: string
  templateName: string
  changes: string[]
  projectId?: string
}

const path = (organizationId: string) => `/api/v1/organizations/${encodeURIComponent(organizationId)}/workspace-templates`

export const templateKeys = {
  workspace: (organizationId: string) => ['organization', organizationId, 'workspace-templates'] as const,
}

export const listTemplates = (organizationId: string) => requestJson<TemplatesResponse>(path(organizationId))

export const applyTemplate = (organizationId: string, template: string) =>
  requestJson<Applied>(`${path(organizationId)}/apply`, { method: 'POST', json: { template } })

export const saveTemplate = (organizationId: string, input: { name: string; description: string; projectId?: string }) =>
  requestJson<WorkspaceTemplate>(path(organizationId), { method: 'POST', json: input })

export const removeTemplate = (organizationId: string, templateId: string) =>
  requestJson<void>(`${path(organizationId)}/${encodeURIComponent(templateId)}`, { method: 'DELETE' })

/** After a template is used: everything it can touch reads again. */
export function refreshAfterTemplate(queryClient: QueryClient, organizationId: string) {
  void queryClient.invalidateQueries({ queryKey: templateKeys.workspace(organizationId) })
  void queryClient.invalidateQueries({ queryKey: ['organization', organizationId, 'modules'] })
  void queryClient.invalidateQueries({ queryKey: roleKeys.roles(organizationId) })
  void queryClient.invalidateQueries({ queryKey: onboardingKeys.root })
  void queryClient.invalidateQueries({ queryKey: ['projects'] })
  void refreshLayouts(queryClient)
}
