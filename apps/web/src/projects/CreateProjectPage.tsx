import { lazy, Suspense, useMemo, useRef, useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useNavigate, useSearchParams } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useWorkspace } from '../app/workspace'
import { useAuthenticatedSession } from '../auth/session'
import { useWorkspaceExperience } from '../experience/layout'
import { useShellPage } from '../shell/shellStore'
import { Banner, Button, ButtonLink, ErrorState, PageHeader, SelectField, TextField, describeError, showToast } from '../ui'
import { listProjectTemplates, viewKeys } from '../views/api'
import { useCreateProject, useProject, useProjects, useSwitchProject } from './hooks'
import { projectKeyPattern, suggestKey } from './projectKey'
import '../styles/pages.css'
import { here, spaceHref } from '../app/href'

// The preset cards bring the layout registry, so they load in their own chunk.
const PresetPicker = lazy(() => import('../experience/PresetPicker'))

function newIdempotencyKey() {
  return crypto.randomUUID()
}

export function CreateProjectPage() {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  const createProject = useCreateProject()
  const switchProject = useSwitchProject()
  const errorRef = useRef<HTMLDivElement>(null)
  const creatableOrganizations = useMemo(
    () => session.organizations.filter((organization) => ['owner', 'admin'].includes(organization.role.toLocaleLowerCase())),
    [session.organizations],
  )
  const [name, setName] = useState('')
  const [typedKey, setTypedKey] = useState<string | null>(null)
  // The new project goes in the workspace in the address when the person can
  // create there; switching workspace on this page moves the form with it.
  const addressed = useWorkspace()?.id ?? null
  const preferredId = (creatableOrganizations.find((organization) => organization.id === addressed) ?? creatableOrganizations[0])?.id ?? ''
  const [organizationId, setOrganizationId] = useState(preferredId)
  const [followed, setFollowed] = useState(addressed)
  const [idempotencyKey] = useState(newIdempotencyKey)
  const [error, setError] = useState<string | null>(null)
  const [templateId, setTemplateId] = useState('')
  // A preset for the new project's layout; none follows the workspace's default.
  const [preset, setPreset] = useState<string | null>(null)
  // "New project inside" opens this page with the parent in the address.
  const [search] = useSearchParams()
  const asked = search.get('inside') ?? ''
  const [parentId, setParentId] = useState(asked)
  // The list holds a person's first hundred projects; the one asked for is read on its own.
  const askedParent = useProject(asked)
  const [people, setPeople] = useState<'parent' | 'own'>('parent')
  if (followed !== addressed) {
    setFollowed(addressed)
    setOrganizationId(preferredId)
    setTemplateId('')
    setPreset(null)
    setParentId('')
  }
  // A layout that hides projects inside projects hides these two fields too.
  // Coming from a project's "New project inside" shows them whatever the workspace's layout says.
  const hidden = !useWorkspaceExperience(organizationId).shows('area.subprojects')
  const nests = !hidden || Boolean(asked)
  // A project can go inside any project of this workspace you manage.
  const projects = useProjects()
  const listed = projects.data?.projects ?? []
  const parents = [...listed, ...(askedParent.data && !listed.some((project) => project.id === asked) ? [askedParent.data] : [])]
    .filter((project) => !project.archivedAt && project.capabilities.manage && project.kind === 'project')
    .sort((left, right) => left.name.localeCompare(right.name))
  const parent = parents.find((project) => project.id === parentId && project.organizationId === organizationId)
  // The one asked for can't hold it (not yours to manage, archived, another
  // workspace): say so, so it isn't quietly made on its own.
  if (parentId && !parent && !askedParent.isPending && !projects.isPending) {
    setParentId('')
    setError('This project can’t go inside the one you came from. It will stand on its own unless you pick another under Inside.')
  }
  const templates = useQuery({
    queryKey: viewKeys.projectTemplates(organizationId),
    queryFn: () => listProjectTemplates(organizationId),
    enabled: Boolean(organizationId),
  })
  const template = templates.data?.templates.find((candidate) => candidate.id === templateId)
  const key = typedKey ?? suggestKey(name)
  const normalizedKey = key.trim().toLocaleUpperCase()
  const keyValid = projectKeyPattern.test(normalizedKey)
  const validation = !name.trim()
    ? 'Give the project a name.'
    : name.trim().length > 120
      ? 'Project names can be up to 120 characters.'
      : !keyValid
        ? 'Use 2–10 letters or numbers for the key, starting with a letter.'
        : !organizationId
          ? 'There’s no workspace to create this project in.'
          : parentId && !parent
            ? 'Still finding the project this goes inside. Try again in a moment.'
            : null
  const canCreate = session.user.capabilities.createProject && creatableOrganizations.length > 0

  useShellPage({ mainClassName: 'aw-main--page' })

  if (!canCreate) {
    return (
      <>
        <div className="aw-page aw-page--reading">
          <ErrorState
            action={
              <ButtonLink to={here('/home')} variant="primary">
                Go to Home
              </ButtonLink>
            }
            title="You can’t create projects here"
          >
            Only workspace owners and admins can create projects. Ask one of them to create it, or to change your role.
          </ErrorState>
        </div>
      </>
    )
  }

  const focusError = () => window.requestAnimationFrame(() => errorRef.current?.focus())

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (validation) {
      setError(validation)
      focusError()
      return
    }
    setError(null)
    try {
      const project = await createProject.mutateAsync({
        organizationId,
        key: normalizedKey,
        name: name.trim(),
        ...(template ? { templateId: template.id } : {}),
        ...(parent ? { parentId: parent.id, people } : {}),
        idempotencyKey,
      })
      if (preset) {
        try {
          const { saveProjectLayout } = await import('../experience/api')
          await saveProjectLayout(project.id, { preset }, 0)
        } catch {
          showToast({ tone: 'danger', title: 'The project’s layout wasn’t set', description: 'Pick it in the project’s settings, under Layout.' })
        }
      }
      try {
        const switched = await switchProject.mutateAsync({
          projectId: project.id,
          expectedVersion: project.navigation.version,
          navigation: project.navigation,
        })
        navigate(switched.href, { replace: true })
      } catch {
        navigate(spaceHref(project.id), { replace: true })
      }
    } catch (reason) {
      setError(isApiError(reason) ? reason.message : `The project wasn’t created. ${describeError(reason).message}`)
      focusError()
    }
  }

  const organizationName = creatableOrganizations[0]?.name
  return (
    <>
      <div className="aw-page aw-page--reading">
        <PageHeader icon="plus" title="New project" />
        {error && (
          <Banner className="aw-page__banner" ref={errorRef} tabIndex={-1} tone="danger">
            {error}
          </Banner>
        )}
        <form className="aw-form aw-card aw-formcard" noValidate onSubmit={submit}>
          {creatableOrganizations.length > 1 && (
            <SelectField
              id="project-organization"
              label="Workspace"
              onChange={(event) => setOrganizationId(event.target.value)}
              value={organizationId}
            >
              {creatableOrganizations.map((organization) => (
                <option key={organization.id} value={organization.id}>
                  {organization.name}
                </option>
              ))}
            </SelectField>
          )}
          <TextField
            autoFocus
            hint={creatableOrganizations.length === 1 && organizationName ? `It will belong to ${organizationName}.` : undefined}
            id="project-name"
            label="Project name"
            maxLength={120}
            onChange={(event) => setName(event.target.value)}
            placeholder="e.g. Trident"
            required
            value={name}
          />
          <TextField
            error={key && !keyValid ? 'Use 2–10 letters or numbers, starting with a letter.' : undefined}
            hint={
              keyValid
                ? `Tasks will be numbered ${normalizedKey}-1, ${normalizedKey}-2 and so on.`
                : 'A short code for tasks, like TRI for Trident.'
            }
            id="project-key"
            label="Project key"
            maxLength={10}
            mono
            onChange={(event) => setTypedKey(event.target.value.toLocaleUpperCase())}
            placeholder="TRI"
            required
            spellCheck={false}
            value={key}
          />
          {nests && parents.some((project) => project.organizationId === organizationId) ? (
            <SelectField
              hint={
                parent
                  ? `It shows in ${parent.name}’s sidebar, with its own tasks and channels.`
                  : 'Put it inside another project when it is one part of that project’s work.'
              }
              id="project-inside"
              label="Inside"
              onChange={(event) => setParentId(event.target.value)}
              value={parent?.id ?? ''}
            >
              <option value="">Nothing, it stands on its own</option>
              {parents
                .filter((project) => project.organizationId === organizationId)
                .map((project) => (
                  <option key={project.id} value={project.id}>
                    {project.name}
                  </option>
                ))}
            </SelectField>
          ) : null}
          {nests && parent ? (
            <SelectField
              hint={
                people === 'parent'
                  ? `Everyone in ${parent.name} is here too, with the same role, and stays in step as people join or leave. You can add more people here.`
                  : `Only you to start with. People in ${parent.name} can’t see it until you add them.`
              }
              id="project-people"
              label="People"
              onChange={(event) => setPeople(event.target.value === 'own' ? 'own' : 'parent')}
              value={people}
            >
              <option value="parent">The same people as {parent.name}</option>
              <option value="own">Only people I add</option>
            </SelectField>
          ) : null}
          {templates.data?.templates.length ? (
            <SelectField
              hint={
                template
                  ? `Starts with its ${[
                      template.contains.statuses.length ? 'workflow' : '',
                      template.contains.labels ? 'labels' : '',
                      template.contains.channels.length ? 'channels' : '',
                      template.contains.settings ? 'defaults' : '',
                      template.contains.taskTemplates ? 'task templates' : '',
                    ]
                      .filter(Boolean)
                      .join(', ')
                      .replace(/, ([^,]*)$/, ' and $1')}.`
                  : 'Start from the default workflow, or from a setup saved from another project.'
              }
              id="project-template"
              label="Template"
              onChange={(event) => setTemplateId(event.target.value)}
              value={templateId}
            >
              <option value="">No template</option>
              {templates.data.templates.map((candidate) => (
                <option key={candidate.id} value={candidate.id}>
                  {candidate.name}
                </option>
              ))}
            </SelectField>
          ) : null}
          {organizationId ? (
            <Suspense fallback={null}>
              <PresetPicker onChange={setPreset} organizationId={organizationId} value={preset} />
            </Suspense>
          ) : null}
          <div className="aw-form__actions">
            <Button onClick={() => navigate(parent ? spaceHref(parent.id) : here('/home'))} variant="secondary">
              Cancel
            </Button>
            <Button busy={createProject.isPending} disabled={Boolean(validation)} type="submit" variant="primary">
              Create project
            </Button>
          </div>
        </form>
      </div>
    </>
  )
}
