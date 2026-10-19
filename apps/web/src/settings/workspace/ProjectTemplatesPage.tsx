import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { toastError } from '../../app/mutations'
import { Button, ButtonLink, Checkbox, EmptyState, ErrorState, Icon, ListSkeleton, PageHeader, Row, RowList, Section, countLabel, showToast } from '../../ui'
import { listProjectTemplates, updateProjectTemplate, viewKeys, type ProjectTemplate } from '../../views/api'
import { projectSettingsHref } from '../paths'
import type { SettingsPageProps } from '../types'

/** Workspace › Project templates: the setups new projects can start from. */
export default function ProjectTemplatesPage({ projects, workspace }: SettingsPageProps) {
  const organizationId = workspace?.id ?? ''
  const queryClient = useQueryClient()
  const [showArchived, setShowArchived] = useState(false)
  const templates = useQuery({
    queryKey: viewKeys.projectTemplates(organizationId, showArchived),
    queryFn: () => listProjectTemplates(organizationId, showArchived),
    enabled: Boolean(organizationId),
  })
  const refresh = () => queryClient.invalidateQueries({ queryKey: ['organization', organizationId, 'project-templates'] })
  const archive = useMutation({
    mutationFn: (input: { template: ProjectTemplate; archived: boolean; undo?: boolean }) =>
      updateProjectTemplate(input.template, { archived: input.archived }, crypto.randomUUID()),
    onSuccess: (template, input) => {
      if (input.undo) return
      showToast({
        title: template.archived ? `${template.name} archived` : `${template.name} restored`,
        onUndo: () => archive.mutate({ template, archived: !template.archived, undo: true }),
      })
    },
    onError: (error, input) => toastError(input.archived ? 'The template wasn’t archived' : 'The template wasn’t restored', error),
    onSettled: refresh,
  })
  if (!workspace) return null
  // Somewhere to save one from: a project here that you manage.
  const source = projects.find((project) => project.organizationId === workspace.id && project.capabilities.manage && !project.archivedAt)

  return (
    <>
      <PageHeader icon="copy" title="Project templates">
        Setups a new project in {workspace.name} can start from: its workflow, labels, channels, defaults and task templates.
        Save one from a project’s Templates settings.
      </PageHeader>
      <Checkbox checked={showArchived} label="Show archived templates" onChange={setShowArchived} />
      {templates.isPending ? <ListSkeleton label="Loading project templates" rows={2} /> : null}
      {templates.isError ? (
        <ErrorState error={templates.error} onRetry={() => void templates.refetch()} title="Project templates didn’t load" />
      ) : null}
      {templates.data ? (
        <Section id="project-templates-heading" title="Templates">
          {templates.data.templates.length ? (
            <RowList label="Project templates">
              {templates.data.templates.map((template) => (
                <Row
                  actions={
                    template.canEdit ? (
                      <Button
                        disabled={archive.isPending && archive.variables?.template.id === template.id}
                        onClick={() => archive.mutate({ template, archived: !template.archived })}
                        size="sm"
                        variant="ghost"
                      >
                        {template.archived ? 'Restore' : 'Archive'}
                      </Button>
                    ) : null
                  }
                  key={template.id}
                  leading={
                    <span className="aw-row__tile">
                      <Icon name={template.archived ? 'archive' : 'copy'} size={16} />
                    </span>
                  }
                  meta={
                    <span className="aw-row__metaline aw-wrap">
                      {template.description ? <span>{template.description}</span> : null}
                      {template.contains.statuses.length ? <span>{countLabel(template.contains.statuses.length, 'status', 'statuses')}</span> : null}
                      {template.contains.labels ? <span>{countLabel(template.contains.labels, 'label')}</span> : null}
                      {template.contains.channels.length ? <span>{template.contains.channels.map((channel) => `#${channel}`).join(' ')}</span> : null}
                      {template.contains.taskTemplates ? <span>{countLabel(template.contains.taskTemplates, 'task template')}</span> : null}
                      {template.contains.settings ? <span>{countLabel(template.contains.settings, 'default')}</span> : null}
                      {template.sourceProjectName ? <span>From {template.sourceProjectName}</span> : null}
                      {template.createdByName ? <span>By {template.createdByName}</span> : null}
                      {template.archived ? <span>Archived</span> : null}
                    </span>
                  }
                  title={template.name}
                />
              ))}
            </RowList>
          ) : (
            <EmptyState
              action={
                source ? (
                  <ButtonLink to={projectSettingsHref(source.id, 'templates')}>Open {source.name} templates</ButtonLink>
                ) : undefined
              }
              icon="copy"
              title="No project templates yet"
            >
              Open a project’s settings, go to Templates and choose Save as project template.
            </EmptyState>
          )}
        </Section>
      ) : null}
    </>
  )
}
