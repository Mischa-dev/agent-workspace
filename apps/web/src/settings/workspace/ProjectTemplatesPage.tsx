import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { toastError } from '../../app/mutations'
import { Button, Checkbox, EmptyState, ErrorState, Icon, ListSkeleton, PageHeader, Row, RowList, Section, showToast } from '../../ui'
import { listProjectTemplates, updateProjectTemplate, viewKeys, type ProjectTemplate } from '../../views/api'
import type { SettingsPageProps } from '../types'

/** Workspace › Project templates: the setups new Projects can start from. */
export default function ProjectTemplatesPage({ workspace }: SettingsPageProps) {
  const organizationId = workspace?.id ?? ''
  const queryClient = useQueryClient()
  const [showArchived, setShowArchived] = useState(false)
  const templates = useQuery({
    queryKey: viewKeys.projectTemplates(organizationId, showArchived),
    queryFn: () => listProjectTemplates(organizationId, showArchived),
    enabled: Boolean(organizationId),
  })
  const archive = useMutation({
    mutationFn: (template: ProjectTemplate) => updateProjectTemplate(template, { archived: !template.archived }, crypto.randomUUID()),
    onSuccess: (template) => showToast({ title: template.archived ? `“${template.name}” archived` : `“${template.name}” restored` }),
    onError: (error) => toastError('That didn’t save', error),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ['organization', organizationId, 'project-templates'] }),
  })
  if (!workspace) return null

  return (
    <>
      <PageHeader icon="list" title="Project templates">
        Setups a new Project in {workspace.name} can start from: its workflow, labels, channels, defaults and task templates.
        Save one from a Project’s Templates settings.
      </PageHeader>
      <Checkbox checked={showArchived} label="Show archived templates" onChange={setShowArchived} />
      {templates.isPending ? <ListSkeleton label="Loading Project templates" rows={2} /> : null}
      {templates.isError ? (
        <ErrorState action={<Button onClick={() => void templates.refetch()}>Try again</Button>} title="Project templates didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : null}
      {templates.data ? (
        <Section id="project-templates-heading" title="Templates">
          {templates.data.templates.length ? (
            <RowList label="Project templates">
              {templates.data.templates.map((template) => (
                <Row
                  actions={
                    template.canEdit ? (
                      <Button onClick={() => archive.mutate(template)} size="sm" variant="ghost">
                        {template.archived ? 'Restore' : 'Archive'}
                      </Button>
                    ) : null
                  }
                  key={template.id}
                  leading={
                    <span className="aw-row__tile">
                      <Icon name={template.archived ? 'archive' : 'list'} size={16} />
                    </span>
                  }
                  meta={
                    <span className="aw-row__metaline aw-wrap">
                      {template.description ? <span>{template.description}</span> : null}
                      {template.contains.statuses.length ? <span>{template.contains.statuses.length} statuses</span> : null}
                      {template.contains.labels ? <span>{template.contains.labels} labels</span> : null}
                      {template.contains.channels.length ? <span>{template.contains.channels.map((channel) => `#${channel}`).join(' ')}</span> : null}
                      {template.contains.taskTemplates ? <span>{template.contains.taskTemplates} task templates</span> : null}
                      {template.contains.settings ? <span>{template.contains.settings} defaults</span> : null}
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
            <EmptyState icon="list" title="No Project templates yet">
              Open a Project’s settings, go to Templates and choose Save as Project template.
            </EmptyState>
          )}
        </Section>
      ) : null}
    </>
  )
}
