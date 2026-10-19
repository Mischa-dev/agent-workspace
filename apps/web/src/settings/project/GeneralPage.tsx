import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { requestJson } from '../../api/client'
import { isApiError } from '../../api/errors'
import { queryKeys } from '../../api/queryKeys'
import { describeError } from '../../app/mutations'
import { roleLabel } from '../../projects/format'
import { useArchiveProject, useProject } from '../../projects/hooks'
import { ProjectInitials } from '../../projects/ProjectInitials'
import type { Project } from '../../projects/types'
import { Banner, Button, DetailList, PageHeader, Section, TextField, showToast } from '../../ui'
import { markArchiving } from '../archiving'
import type { SettingsPageProps } from '../types'

const renameProject = (projectId: string, name: string, expectedVersion: number) =>
  requestJson<Project>(`/api/v1/projects/${encodeURIComponent(projectId)}`, {
    method: 'PATCH',
    json: { name, expectedVersion },
  })

/** Project › General: about the project, its name, and archiving it. */
export default function GeneralPage({ project: given, workspace }: SettingsPageProps) {
  const projectId = given?.id ?? ''
  const project = useProject(projectId)
  const archive = useArchiveProject()
  const navigate = useNavigate()
  const errorRef = useRef<HTMLDivElement>(null)
  const [confirmation, setConfirmation] = useState('')
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])
  const current = project.data ?? given
  if (!current) return null

  const submitArchive = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (confirmation !== current.key) return
    setError(null)
    markArchiving(current.id, true)
    try {
      await archive.mutateAsync({ projectId: current.id, expectedVersion: current.version })
      navigate('/home', {
        replace: true,
        state: { notice: `${current.name} was archived. Everything in it was kept.` },
      })
    } catch (reason) {
      // Only a failure clears it: after success the page leaves for Home.
      markArchiving(current.id, false)
      if (isApiError(reason) && reason.code === 'version_conflict') {
        await project.refetch()
        setError('Someone changed this project while you were here. Check the details above and try again.')
      } else {
        setError(`The project wasn’t archived. ${describeError(reason).message}`)
      }
    }
  }

  return (
    <>
      <PageHeader mark={<ProjectInitials project={current} size={40} />} title="General">
        {current.name} · {current.key}
      </PageHeader>
      <Section id="project-details-heading" title="About this project">
        <DetailList
          items={[
            { label: 'Name', value: current.name },
            { label: 'Key', value: <code className="aw-inline-code">{current.key}</code> },
            { label: 'Your role', value: roleLabel(current.role) },
            ...(workspace ? [{ label: 'Workspace', value: workspace.name }] : []),
          ]}
        />
        {current.role === 'viewer' && (
          <Banner role="note" tone="info">
            Viewer access is read-only. You can look at the project and its repositories, but members, agents, runs and
            GitHub are managed by members and admins.
          </Banner>
        )}
      </Section>
      {current.capabilities.manage && !current.archivedAt && <RenameSection project={current} />}
      {current.capabilities.archive && !current.archivedAt && (
        <Section
          description="Archiving takes it out of everyone’s sidebar and stops new messages, tasks and runs. Everything already in it is kept and stays readable."
          id="archive-project-heading"
          title="Archive this project"
        >
          <form className="aw-danger" onSubmit={submitArchive}>
            {error && (
              <Banner ref={errorRef} tabIndex={-1} tone="danger">
                {error}
              </Banner>
            )}
            <TextField
              autoComplete="off"
              id="archive-confirmation"
              label={
                <>
                  Type <strong>{current.key}</strong> to confirm
                </>
              }
              mono
              onChange={(event) => setConfirmation(event.target.value)}
              spellCheck={false}
              value={confirmation}
            />
            <div>
              <Button busy={archive.isPending} disabled={confirmation !== current.key} icon="archive" type="submit" variant="danger">
                Archive project
              </Button>
            </div>
          </form>
        </Section>
      )}
    </>
  )
}

/** The project's name. Its key stays, so task numbers and links keep working. */
function RenameSection({ project }: { project: Project }) {
  const queryClient = useQueryClient()
  const [draft, setDraft] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const name = draft ?? project.name
  const trimmed = name.trim()
  const rename = useMutation({
    mutationFn: () => renameProject(project.id, trimmed, project.version),
    onSuccess: (renamed) => {
      setDraft(null)
      setError(null)
      queryClient.setQueryData(queryKeys.project(renamed.id), renamed)
      void queryClient.invalidateQueries({ queryKey: ['projects'] })
      showToast({ tone: 'success', title: `Renamed to ${renamed.name}` })
    },
    onError: async (reason) => {
      if (isApiError(reason) && reason.code === 'version_conflict') {
        await queryClient.invalidateQueries({ queryKey: queryKeys.project(project.id) })
        setError('Someone changed this project while you were here. Its current name is shown; try again if you still want to change it.')
        setDraft(null)
      } else {
        setError(`The project wasn’t renamed. ${describeError(reason).message}`)
      }
    },
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (trimmed && trimmed !== project.name) rename.mutate()
  }
  return (
    <Section
      description={`Everyone in the project sees this name. Its key, ${project.key}, stays the same, so task numbers and links keep working.`}
      id="rename-project-heading"
      title="Name"
    >
      <form className="aw-inline-form" onSubmit={submit}>
        <TextField
          id="project-name"
          label="Project name"
          maxLength={120}
          onChange={(event) => setDraft(event.target.value)}
          required
          value={name}
        />
        <Button busy={rename.isPending} disabled={!trimmed || trimmed === project.name} type="submit" variant="primary">
          Rename
        </Button>
      </form>
      {error && <Banner tone="warning">{error}</Banner>}
    </Section>
  )
}
