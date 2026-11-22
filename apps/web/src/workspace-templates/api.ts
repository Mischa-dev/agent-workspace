import type { QueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { callOp, ranOrThrow } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
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
  layoutChanges?: Record<string, boolean>
  layoutWords?: Record<string, { one: string; many: string }>
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

/** Uses a template, in the workspace's own space. */
export async function applyTemplate(organizationId: string, template: string): Promise<Applied> {
  const applied = ranOrThrow(
    await callOp('workspace_template_apply', { projectId: 'workspace', template }, { workspaceId: organizationId }),
    'Using the template',
  )
  return { ...applied, projectId: applied.projectId ?? undefined }
}

/** Saves the workspace as a template; with a project, carrying its channels and collections. */
export async function saveTemplate(
  organizationId: string,
  input: { name: string; description: string; projectId?: string },
): Promise<WorkspaceTemplate> {
  const saved = ranOrThrow(
    await callOp(
      'workspace_template_save',
      {
        projectId: input.projectId ?? 'workspace',
        includeProject: Boolean(input.projectId),
        name: input.name,
        description: input.description,
      },
      { workspaceId: organizationId },
    ),
    'Saving the template',
  )
  if (!saved.template) throw new Error('The template wasn’t saved.')
  return saved.template as WorkspaceTemplate
}

export async function removeTemplate(organizationId: string, templateId: string) {
  ranOrThrow(
    await callOp('workspace_template_save', { projectId: 'workspace', remove: templateId }, { workspaceId: organizationId }),
    'Removing the template',
  )
}

/** After a template is used: everything it can touch reads again. */
export function refreshAfterTemplate(queryClient: QueryClient, organizationId: string) {
  void queryClient.invalidateQueries({ queryKey: templateKeys.workspace(organizationId) })
  void queryClient.invalidateQueries({ queryKey: queryKeys.workspaceModules(organizationId) })
  void queryClient.invalidateQueries({ queryKey: roleKeys.roles(organizationId) })
  void queryClient.invalidateQueries({ queryKey: onboardingKeys.root })
  void queryClient.invalidateQueries({ queryKey: queryKeys.projectLists })
  void refreshLayouts(queryClient)
}
