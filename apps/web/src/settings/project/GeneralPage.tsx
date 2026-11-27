import { here } from '../../app/href'
import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { can } from '../../auth/can'
import { callOp, isVersionConflict } from '../../api/ops'
import { queryKeys } from '../../api/queryKeys'
import { describeError } from '../../app/mutations'
import { useProjectExperience } from '../../experience/layout'
import { roleLabel } from '../../projects/format'
import { placeProject, renameProject } from '../../projects/api'
import { useArchiveProject, useProject, useProjects } from '../../projects/hooks'
import { ProjectInitials } from '../../projects/ProjectInitials'
import type { Project } from '../../projects/types'
import { Banner, Button, DetailList, PageHeader, Section, SelectField, TextField, showToast } from '../../ui'
import { markArchiving } from '../archiving'
import type { SettingsPageProps } from '../types'
import '../../styles/pages.css'

/** Project › General: about the project, its name, and archiving it. */
export default function GeneralPage({ project: given, workspace }: SettingsPageProps) {
  const projectId = given?.id ?? ''
  const project = useProject(projectId)
  const experience = useProjectExperience(projectId)
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
      navigate(here('/home'), {
        replace: true,
        state: { notice: `${current.name} was archived. Everything in it was kept.` },
      })
    } catch (reason) {
      // Only a failure clears it: after success the page leaves for Home.
      markArchiving(current.id, false)
      if (isVersionConflict(reason)) {
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
        {!can(current, 'tasks.edit') && (
          <Banner role="note" tone="info">
            Your role here is read-only. You can look at the project and its repositories; members, agents, runs and
            GitHub are managed by people whose role allows it.
          </Banner>
        )}
      </Section>
      {current.capabilities.manage && !current.archivedAt && <RenameSection project={current} />}
      {current.capabilities.manage && !current.archivedAt && <IconSection project={current} />}
      {current.capabilities.manage &&
        !current.archivedAt &&
        current.kind === 'project' &&
        experience.shows('area.subprojects') && <PlaceSection project={current} />}
      {current.capabilities.archive && !current.archivedAt && (
        <Section
          description={`Archiving takes it out of everyone’s sidebar and stops new ${stopped(experience.moduleOn)}. Projects inside it are archived with it. Everything already in them is kept and stays readable.`}
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
/** What archiving stops, in the words of the modules that are on (W1.39). */
function stopped(moduleOn: (module: string) => boolean) {
  const things = ['messages', moduleOn('tasks') ? 'tasks' : null, moduleOn('runs') ? 'runs' : null].filter((thing): thing is string => thing !== null)
  return things.length > 1 ? `${things.slice(0, -1).join(', ')} and ${things[things.length - 1]}` : things[0]
}

function RenameSection({ project }: { project: Project }) {
  const tasksOn = useProjectExperience(project.id).moduleOn('tasks')
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
      void queryClient.invalidateQueries({ queryKey: queryKeys.projectLists })
      showToast({ tone: 'success', title: `Renamed to ${renamed.name}` })
    },
    onError: async (reason) => {
      if (isVersionConflict(reason)) {
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
      description={`Everyone in the project sees this name. Its key, ${project.key}, stays the same, so ${tasksOn ? 'task numbers and links' : 'links'} keep working.`}
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

/** The project's icon: an emoji shown beside its name in the rail, the sidebar and the command menu. */
function IconSection({ project }: { project: Project }) {
  const queryClient = useQueryClient()
  const [draft, setDraft] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const icon = draft ?? project.icon ?? ''
  const trimmed = icon.trim()
  const save = useMutation({
    mutationFn: (next: string) =>
      callOp(
        'space_update',
        { spaceId: project.id, icon: next },
        { workspaceId: project.organizationId, expectedVersion: project.version },
      ),
    onSuccess: async (_, next) => {
      setDraft(null)
      setError(null)
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.project(project.id) }),
        queryClient.invalidateQueries({ queryKey: queryKeys.projectLists }),
      ])
      showToast({ tone: 'success', title: next ? 'Icon saved' : 'Icon removed' })
    },
    onError: async (reason) => {
      if (isVersionConflict(reason)) {
        await queryClient.invalidateQueries({ queryKey: queryKeys.project(project.id) })
        setError('Someone changed this project while you were here. Try again.')
        setDraft(null)
      } else {
        setError(`The icon wasn’t saved. ${describeError(reason).message}`)
      }
    },
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (trimmed !== (project.icon ?? '')) save.mutate(trimmed)
  }
  return (
    <Section
      description="An emoji shown in place of the key on the project’s tile in the rail, beside its name in the sidebar, and in the command menu."
      id="project-icon-heading"
      title="Icon"
    >
      <form className="aw-inline-form" onSubmit={submit}>
        <TextField
          id="project-icon"
          label="Emoji"
          maxLength={16}
          onChange={(event) => setDraft(event.target.value)}
          value={icon}
        />
        <Button busy={save.isPending} disabled={trimmed === (project.icon ?? '')} type="submit" variant="primary">
          Save
        </Button>
        {project.icon ? (
          <Button busy={save.isPending} onClick={() => save.mutate('')} type="button">
            Remove
          </Button>
        ) : null}
      </form>
      {error && <Banner tone="warning">{error}</Banner>}
    </Section>
  )
}

/** Where the project sits: inside another project or on its own, and whose people it has. */
function PlaceSection({ project }: { project: Project }) {
  const queryClient = useQueryClient()
  const projects = useProjects()
  const open = (projects.data?.projects ?? []).filter((candidate) => !candidate.archivedAt && candidate.organizationId === project.organizationId)
  // Not itself, and nothing that is already inside it.
  const under = (candidate: Project) => {
    let at: Project | undefined = candidate
    for (let step = 0; at && step < 8; step += 1) {
      if (at.id === project.id) return true
      const parentId: string | null | undefined = at.parentId
      at = parentId ? open.find((other) => other.id === parentId) : undefined
    }
    return false
  }
  const parents = open
    .filter((candidate) => candidate.kind === 'project' && candidate.capabilities.manage && !under(candidate))
    .sort((left, right) => left.name.localeCompare(right.name))
  const [draftParent, setDraftParent] = useState<string | null>(null)
  const [draftPeople, setDraftPeople] = useState<'parent' | 'own' | null>(null)
  const [error, setError] = useState<string | null>(null)
  const savedParent = project.parentId ?? ''
  const savedPeople = project.peopleFromParent ? 'parent' : 'own'
  const parentId = draftParent ?? savedParent
  // A project newly put inside another takes its people unless you say otherwise.
  const people = draftPeople ?? (parentId && parentId !== savedParent ? 'parent' : savedPeople)
  const parent = open.find((candidate) => candidate.id === parentId)
  const changed = parentId !== savedParent || (Boolean(parentId) && people !== savedPeople)
  const save = useMutation({
    mutationFn: () =>
      placeProject(
        project.id,
        {
          ...(parentId !== savedParent ? { parentId: parentId || null } : {}),
          ...(parentId && people !== savedPeople ? { people } : {}),
        },
        project.version,
      ),
    onSuccess: (placed) => {
      setDraftParent(null)
      setDraftPeople(null)
      setError(null)
      queryClient.setQueryData(queryKeys.project(placed.id), placed)
      void queryClient.invalidateQueries({ queryKey: queryKeys.projectLists })
      void queryClient.invalidateQueries({ queryKey: queryKeys.projectTrees })
      showToast({ tone: 'success', title: placed.parentId && parent ? `Now inside ${parent.name}` : 'Now a project on its own' })
    },
    onError: async (reason) => {
      if (isVersionConflict(reason)) {
        await queryClient.invalidateQueries({ queryKey: queryKeys.project(project.id) })
        setError('Someone changed this project while you were here. Try again.')
        setDraftParent(null)
        setDraftPeople(null)
      } else {
        setError(`The project wasn’t moved. ${describeError(reason).message}`)
      }
    },
  })
  // Inside a project you can’t see or manage: say so, and leave it be.
  if (savedParent && !parents.some((candidate) => candidate.id === savedParent)) {
    return (
      <Section id="project-place-heading" title="Inside">
        <Banner role="note" tone="info">
          This project is inside another one. Someone who manages both can move it.
        </Banner>
      </Section>
    )
  }
  if (!parents.length) return null
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (changed) save.mutate()
  }
  return (
    <Section
      description="A project inside another shows in that project’s sidebar and keeps its own tasks, channels and key."
      id="project-place-heading"
      title="Inside"
    >
      <form className="aw-form" onSubmit={submit}>
        <SelectField id="project-inside" label="Inside" onChange={(event) => setDraftParent(event.target.value)} value={parentId}>
          <option value="">Nothing, it stands on its own</option>
          {parents.map((candidate) => (
            <option key={candidate.id} value={candidate.id}>
              {candidate.name}
            </option>
          ))}
        </SelectField>
        {parent ? (
          <SelectField
            hint={
              people === 'parent'
                ? `Everyone in ${parent.name} is here too, with the same role, and stays in step as people join or leave.`
                : `Only the people added here. If it had ${parent.name}’s people, they stay until you remove them.`
            }
            id="project-people"
            label="People"
            onChange={(event) => setDraftPeople(event.target.value === 'own' ? 'own' : 'parent')}
            value={people}
          >
            <option value="parent">The same people as {parent.name}</option>
            <option value="own">Only people added here</option>
          </SelectField>
        ) : null}
        <div>
          <Button busy={save.isPending} disabled={!changed} type="submit" variant="primary">
            Save
          </Button>
        </div>
      </form>
      {error && <Banner tone="warning">{error}</Banner>}
    </Section>
  )
}
