import { FormEvent, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import {
  archiveRepository,
  listRepositories,
  prepareRepository,
  registerRepository,
  type Repository,
} from './api'

export function RepositorySettings({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient()
  const key = queryKeys.repositories(projectId)
  const repositories = useQuery({
    queryKey: key,
    queryFn: () => listRepositories(projectId),
  })
  const update = (repository: Repository) => {
    queryClient.setQueryData<Awaited<ReturnType<typeof listRepositories>>>(key, (current) => {
      const retained = (current?.repositories ?? []).filter(
        (candidate) => candidate.id !== repository.id,
      )
      return {
        canManage: current?.canManage ?? repository.capabilities.manage,
        repositories: [...retained, repository].sort((left, right) =>
          `${left.owner}/${left.name}`.localeCompare(`${right.owner}/${right.name}`),
        ),
      }
    })
  }
  const prepare = useMutation({
    mutationFn: (repositoryId: string) =>
      prepareRepository(projectId, repositoryId, crypto.randomUUID()),
    onSuccess: update,
  })
  const archive = useMutation({
    mutationFn: (repository: Repository) =>
      archiveRepository(projectId, repository.id, repository.version, crypto.randomUUID()),
    onSuccess: update,
  })
  const [error, setError] = useState<string | null>(null)

  if (repositories.isPending) return <p>Loading repositories…</p>
  if (repositories.isError) return <p role="alert">Repositories could not be loaded.</p>
  const canManage = repositories.data.canManage

  return (
    <div className="repository-settings">
      {error && <p role="alert">{error}</p>}
      {repositories.data.repositories.length ? (
        <div className="repository-list">
          {repositories.data.repositories.map((repository) => (
            <article className="repository-card" key={repository.id}>
              <header>
                <div>
                  <h3>{repository.owner}/{repository.name}</h3>
                  <p>Default branch: {repository.defaultBranch}</p>
                </div>
                <span className="status-chip" data-state={repository.preparationState}>
                  {repository.archivedAt ? 'archived' : repository.preparationState}
                </span>
              </header>
              {repository.preparationFailureCategory && (
                <p>Preparation needs attention: {repository.preparationFailureCategory.replaceAll('_', ' ')}.</p>
              )}
              {repository.capabilities.manage && !repository.archivedAt && (
                <div className="repository-actions">
                  {repository.preparationState !== 'ready' && (
                    <button
                      disabled={prepare.isPending}
                      onClick={() => {
                        setError(null)
                        prepare.mutate(repository.id, {
                          onError: (reason) => setError(errorMessage(reason)),
                        })
                      }}
                      type="button"
                    >
                      {prepare.isPending ? 'Preparing…' : 'Prepare mirror'}
                    </button>
                  )}
                  <button
                    className="danger-button"
                    disabled={archive.isPending}
                    onClick={() => {
                      setError(null)
                      archive.mutate(repository, {
                        onError: (reason) => setError(errorMessage(reason)),
                      })
                    }}
                    type="button"
                  >
                    Archive repository
                  </button>
                </div>
              )}
            </article>
          ))}
        </div>
      ) : (
        <p>No repositories are registered for this Project.</p>
      )}
      {canManage && <RepositoryRegistrationForm onCreated={update} projectId={projectId} />}
    </div>
  )
}

function RepositoryRegistrationForm({
  onCreated,
  projectId,
}: {
  onCreated: (repository: Repository) => void
  projectId: string
}) {
  const [githubRepositoryId, setGithubRepositoryId] = useState('')
  const [owner, setOwner] = useState('')
  const [name, setName] = useState('')
  const [defaultBranch, setDefaultBranch] = useState('main')
  const [trusted, setTrusted] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const registration = useMutation({
    mutationFn: () =>
      registerRepository(projectId, {
        githubRepositoryId: Number(githubRepositoryId),
        owner,
        name,
        cloneUrl: `https://github.com/${owner}/${name}.git`,
        defaultBranch,
        trustAcknowledged: trusted,
        idempotencyKey: crypto.randomUUID(),
      }),
    onSuccess: (repository) => {
      onCreated(repository)
      setGithubRepositoryId('')
      setOwner('')
      setName('')
      setDefaultBranch('main')
      setTrusted(false)
    },
    onError: (reason) => setError(errorMessage(reason)),
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setError(null)
    registration.mutate()
  }
  const valid = Number(githubRepositoryId) > 0 && owner && name && defaultBranch && trusted

  return (
    <form className="repository-registration" onSubmit={submit}>
      <h3>Register GitHub repository</h3>
      <div className="field-grid">
        <div className="field">
          <label htmlFor="github-repository-id">GitHub repository ID</label>
          <input id="github-repository-id" inputMode="numeric" onChange={(event) => setGithubRepositoryId(event.target.value)} value={githubRepositoryId} />
        </div>
        <div className="field">
          <label htmlFor="repository-owner">Owner</label>
          <input id="repository-owner" onChange={(event) => setOwner(event.target.value)} value={owner} />
        </div>
        <div className="field">
          <label htmlFor="repository-name">Repository</label>
          <input id="repository-name" onChange={(event) => setName(event.target.value)} value={name} />
        </div>
        <div className="field">
          <label htmlFor="repository-default-branch">Default branch</label>
          <input id="repository-default-branch" onChange={(event) => setDefaultBranch(event.target.value)} value={defaultBranch} />
        </div>
      </div>
      <label className="trust-acknowledgement">
        <input checked={trusted} onChange={(event) => setTrusted(event.target.checked)} type="checkbox" />
        I trust this repository. Hosted runs execute its code under the dedicated Agent Workspace service account.
      </label>
      {error && <p role="alert">{error}</p>}
      <button disabled={!valid || registration.isPending} type="submit">
        {registration.isPending ? 'Registering…' : 'Register repository'}
      </button>
    </form>
  )
}

function errorMessage(reason: unknown) {
  return isApiError(reason) ? reason.message : 'The repository operation failed.'
}
