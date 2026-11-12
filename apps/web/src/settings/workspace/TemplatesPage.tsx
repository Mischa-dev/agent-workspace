import { useState, type FormEvent } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Package } from 'lucide-react'

import { Button, Icon, PageHeader, Section, SelectField, TextField, describeError, showToast } from '../../ui'
import { saveTemplate, templateKeys } from '../../workspace-templates/api'
import { TemplatePicker } from '../../workspace-templates/TemplatePicker'
import type { SettingsPageProps } from '../types'

// AWD-76: Workspace › Workspace templates. Use a template on the workspace,
// or save the workspace as one: its modules, layout, roles and default role,
// and optionally a project's channels and collections.

export default function TemplatesPage({ projects, workspace }: SettingsPageProps) {
  if (!workspace) return null
  const manages = workspace.role === 'owner' || workspace.role === 'admin'
  const ownProjects = projects.filter((project) => project.organizationId === workspace.id && !project.archivedAt)
  return (
    <>
      <PageHeader
        mark={
          <span className="aw-pagehead__icon">
            <Icon name={Package} size={20} />
          </span>
        }
        title="Workspace templates"
      >
        Ready-made setups for {workspace.name}: which modules are on, the layout everyone starts with, roles and the role new
        people get, and a starter project. Agents can read these and suggest one; owners and admins use them.
      </PageHeader>
      <Section title="Templates">
        <TemplatePicker confirm organizationId={workspace.id} />
      </Section>
      {manages ? <SaveForm key={workspace.id} organizationId={workspace.id} projects={ownProjects} /> : null}
    </>
  )
}

function SaveForm({ organizationId, projects }: { organizationId: string; projects: SettingsPageProps['projects'] }) {
  const queryClient = useQueryClient()
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [projectId, setProjectId] = useState('')
  const save = useMutation({
    mutationFn: () => saveTemplate(organizationId, { name: name.trim(), description: description.trim(), ...(projectId ? { projectId } : {}) }),
    onSuccess: (template) => {
      void queryClient.invalidateQueries({ queryKey: templateKeys.workspace(organizationId) })
      setName('')
      setDescription('')
      setProjectId('')
      showToast({ tone: 'success', title: `${template.name} saved`, description: 'It’s in the list above, for this workspace.' })
    },
  })
  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (name.trim()) save.mutate()
  }
  return (
    <Section
      description="Keeps how this workspace is set up now, to use again later. Members, tasks and messages never go in a template."
      title="Save this workspace as a template"
    >
      <form className="aw-form" onSubmit={submit}>
        <TextField error={save.isError ? describeError(save.error).message : undefined} label="Name" maxLength={60} onChange={(event) => setName(event.target.value)} required value={name} />
        <TextField label="What it’s for" maxLength={300} onChange={(event) => setDescription(event.target.value)} value={description} />
        <SelectField hint="Its channels and collections come too, made empty." label="Starter project" onChange={(event) => setProjectId(event.target.value)} value={projectId}>
          <option value="">None</option>
          {projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name}
            </option>
          ))}
        </SelectField>
        <div>
          <Button busy={save.isPending} disabled={!name.trim()} type="submit" variant="primary">
            Save template
          </Button>
        </div>
      </form>
    </Section>
  )
}
