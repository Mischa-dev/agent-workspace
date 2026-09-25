import { useMemo, useRef, useState, type FormEvent } from 'react'
import { Link, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { Banner, Button, ErrorState, PageHeader, SelectField, TextField } from '../ui'
import { useCreateProject, useSwitchProject } from './hooks'

const projectKeyPattern = /^[A-Z][A-Z0-9]{1,9}$/

function newIdempotencyKey() {
  return crypto.randomUUID()
}

/** "Design review" → "DR", "Trident" → "TRI": a starting point the person can change. */
function suggestKey(name: string) {
  const words = name
    .toLocaleUpperCase()
    .replace(/[^A-Z0-9]+/g, ' ')
    .trim()
    .split(' ')
    .filter(Boolean)
  if (!words.length) return ''
  const initials = words.map((word) => word[0]).join('')
  const key = (words.length > 1 && initials.length >= 2 ? initials : words[0]).replace(/^[0-9]+/, '')
  return key.slice(0, words.length > 1 ? 4 : 3)
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
  const key = typedKey ?? suggestKey(name)
  const normalizedKey = key.trim().toLocaleUpperCase()
  const keyValid = projectKeyPattern.test(normalizedKey)
  const validation = !name.trim()
    ? 'Give the Project a name.'
    : name.trim().length > 120
      ? 'Project names can be up to 120 characters.'
      : !keyValid
        ? 'Use 2–10 letters or numbers for the key, starting with a letter.'
        : !organizationId
          ? 'There’s no organization to create this Project in.'
          : null
  const canCreate = session.user.capabilities.createProject && creatableOrganizations.length > 0

  if (!canCreate) {
    return (
      <AppShell mainClassName="aw-main--page" user={session.user}>
        <div className="aw-page aw-page--narrow">
          <ErrorState
            action={
              <Link className="aw-btn aw-btn--secondary aw-btn--md" to="/home">
                <span className="aw-btn__label">Back to Home</span>
              </Link>
            }
            title="Project creation unavailable"
          >
            Only owners and admins of an organization can create Projects. Ask one of them to create it, or to change
            your role.
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
        idempotencyKey,
      })
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
      setError(isApiError(reason) ? reason.message : 'The Project wasn’t created. Try again.')
      focusError()
    }
  }

  const organizationName = creatableOrganizations[0]?.name
  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-page--form">
        <PageHeader icon="plus" title="Create a Project">
          A Project holds its own channels, threads, work items, repositories and agents.
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
              label="Organization"
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
                ? `Work items will be numbered ${normalizedKey}-1, ${normalizedKey}-2 and so on.`
                : 'A short code for work items, like TRI for Trident.'
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
          <div className="aw-form__actions">
            <Button onClick={() => navigate('/home')} variant="ghost">
              Cancel
            </Button>
            <Button busy={createProject.isPending} disabled={Boolean(validation)} icon="plus" type="submit" variant="primary">
              Create Project
            </Button>
          </div>
        </form>
      </div>
    </AppShell>
  )
}
