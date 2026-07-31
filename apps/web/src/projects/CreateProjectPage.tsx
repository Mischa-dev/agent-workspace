import { FormEvent, useMemo, useRef, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'

import { isApiError } from '../api/errors'
import { useAuthenticatedSession } from '../auth/session'
import { AppShell } from '../shell/AppShell'
import { useCreateProject, useSwitchProject } from './hooks'

const projectKeyPattern = /^[A-Z][A-Z0-9]{1,9}$/

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
    () =>
      session.organizations.filter((organization) =>
        ['owner', 'admin'].includes(organization.role.toLocaleLowerCase()),
      ),
    [session.organizations],
  )
  const [name, setName] = useState('')
  const [key, setKey] = useState('')
  const [organizationId, setOrganizationId] = useState(
    creatableOrganizations[0]?.id ?? '',
  )
  const [idempotencyKey] = useState(newIdempotencyKey)
  const [error, setError] = useState<string | null>(null)
  const normalizedKey = key.trim().toLocaleUpperCase()
  const validation = useMemo(() => {
    if (!name.trim()) return 'Enter a Project name.'
    if (name.trim().length > 120) {
      return 'Project names must be 120 characters or fewer.'
    }
    if (!projectKeyPattern.test(normalizedKey)) {
      return 'Use 2–10 uppercase letters or numbers, beginning with a letter.'
    }
    if (!organizationId) return 'No Organization is available for this Project.'
    return null
  }, [name, normalizedKey, organizationId])
  const canCreate =
    session.user.capabilities.createProject &&
    creatableOrganizations.length > 0

  if (!canCreate) {
    return (
      <AppShell user={session.user}>
        <div className="page-content narrow-page">
          <div className="error-state" role="alert">
            <h1>Project creation unavailable</h1>
            <p>
              You need an Owner or Admin role in an Organization to create a
              Project.
            </p>
            <Link className="secondary-link" to="/home">
              Return to Home
            </Link>
          </div>
        </div>
      </AppShell>
    )
  }

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (validation) {
      setError(validation)
      window.requestAnimationFrame(() => errorRef.current?.focus())
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
        navigate(`/projects/${encodeURIComponent(project.id)}`, {
          replace: true,
        })
      }
    } catch (reason) {
      setError(
        isApiError(reason) ? reason.message : 'The Project could not be created.',
      )
      window.requestAnimationFrame(() => errorRef.current?.focus())
    }
  }

  return (
    <AppShell user={session.user}>
      <div className="page-content narrow-page">
        <header className="page-heading">
          <div>
            <span className="section-label">New environment</span>
            <h1>Create a Project</h1>
            <p>
              Give this workspace a durable name and a short key for future work
              references.
            </p>
          </div>
        </header>
        {error && (
          <div
            className="form-error-summary"
            ref={errorRef}
            role="alert"
            tabIndex={-1}
          >
            {error}
          </div>
        )}
        <form className="project-form" onSubmit={submit}>
          {creatableOrganizations.length > 1 ? (
            <div className="field">
              <label htmlFor="project-organization">Organization</label>
              <select
                id="project-organization"
                onChange={(event) => setOrganizationId(event.target.value)}
                value={organizationId}
              >
                {creatableOrganizations.map((organization) => (
                  <option key={organization.id} value={organization.id}>
                    {organization.name}
                  </option>
                ))}
              </select>
            </div>
          ) : (
            <p className="form-context">
              Organization:{' '}
              <strong>{creatableOrganizations[0]?.name ?? 'Unavailable'}</strong>
            </p>
          )}
          <div className="field">
            <label htmlFor="project-name">Project name</label>
            <input
              autoFocus
              id="project-name"
              maxLength={120}
              onChange={(event) => setName(event.target.value)}
              placeholder="e.g. Trident"
              required
              value={name}
            />
          </div>
          <div className="field">
            <label htmlFor="project-key">Project key</label>
            <input
              aria-describedby="project-key-help"
              id="project-key"
              maxLength={10}
              onChange={(event) => setKey(event.target.value.toLocaleUpperCase())}
              pattern="[A-Z][A-Z0-9]{1,9}"
              placeholder="TRI"
              required
              spellCheck={false}
              value={key}
            />
            <p id="project-key-help">
              2–10 uppercase letters or numbers. Begin with a letter.
            </p>
          </div>
          <div className="form-actions">
            <button
              className="secondary-button"
              onClick={() => navigate('/home')}
              type="button"
            >
              Cancel
            </button>
            <button
              className="primary-button"
              disabled={createProject.isPending || Boolean(validation)}
              type="submit"
            >
              {createProject.isPending ? 'Creating…' : 'Create Project'}
            </button>
          </div>
        </form>
      </div>
    </AppShell>
  )
}
