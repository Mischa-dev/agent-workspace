import { lazy, Suspense, useMemo, useRef, useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { Banner, Button, ButtonLink, ErrorState, PageHeader, SelectField, TextField, describeError, showToast } from '../ui'
import { listProjectTemplates, viewKeys } from '../views/api'
import { useCreateProject, useSwitchProject } from './hooks'
import { projectKeyPattern, suggestKey } from './projectKey'

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
  const [organizationId, setOrganizationId] = useState(creatableOrganizations[0]?.id ?? '')
  const [idempotencyKey] = useState(newIdempotencyKey)
  const [error, setError] = useState<string | null>(null)
  const [templateId, setTemplateId] = useState('')
  // A preset for the new project's layout; none follows the workspace's default.
  const [preset, setPreset] = useState<string | null>(null)
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
          : null
  const canCreate = session.user.capabilities.createProject && creatableOrganizations.length > 0

  if (!canCreate) {
    return (
      <AppShell mainClassName="aw-main--page" user={session.user}>
        <div className="aw-page aw-page--narrow">
          <ErrorState
            action={
              <ButtonLink to="/home" variant="primary">
                Go to Home
              </ButtonLink>
            }
            title="You can’t create projects here"
          >
            Only workspace owners and admins can create projects. Ask one of them to create it, or to change your role.
          </ErrorState>
        </div>
      </AppShell>
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
        navigate(`/projects/${encodeURIComponent(project.id)}`, { replace: true })
      }
    } catch (reason) {
      setError(isApiError(reason) ? reason.message : `The project wasn’t created. ${describeError(reason).message}`)
      focusError()
    }
  }

  const organizationName = creatableOrganizations[0]?.name
  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-page--form">
        <PageHeader icon="plus" title="New project">
          A project holds its own channels, threads, tasks, repositories and agents.
          {creatableOrganizations.length === 1 && organizationName ? ` It will belong to ${organizationName}.` : ''}
        </PageHeader>
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
            <Button onClick={() => navigate('/home')} variant="secondary">
              Cancel
            </Button>
            <Button busy={createProject.isPending} disabled={Boolean(validation)} type="submit" variant="primary">
              Create project
            </Button>
          </div>
        </form>
      </div>
    </AppShell>
  )
}
