import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { humanize } from '../projects/format'
import { Banner, Button, Checkbox, ErrorState, Icon, ListSkeleton, Row, RowList, StatusChip, TextField } from '../ui'
import { archiveRepository, listRepositories, prepareRepository, registerRepository, type Repository } from './api'

function RepositoryState({ repository }: { repository: Repository }) {
  if (repository.archivedAt) return <StatusChip icon="archive">Archived</StatusChip>
  if (repository.preparationState === 'ready') {
    return (
      <StatusChip icon="circle-check" tone="success">
        Ready
      </StatusChip>
    )
  }
  if (repository.preparationState === 'failed') {
    return (
      <StatusChip icon="triangle-alert" tone="danger">
        Needs attention
      </StatusChip>
    )
  }
  return (
    <StatusChip icon="clock" tone="warning">
      Not set up
    </StatusChip>
  )
}

export function RepositorySettings({
  projectId,
  showRegistration = true,
}: {
  projectId: string
  /** The form for entering a repository by hand; hosts with the GitHub connect flow use the picker instead. */
  showRegistration?: boolean
}) {
  const queryClient = useQueryClient()
  const key = queryKeys.repositories(projectId)
  const repositories = useQuery({
    queryKey: key,
    queryFn: () => listRepositories(projectId),
  })
  const update = (repository: Repository) => {
    queryClient.setQueryData<Awaited<ReturnType<typeof listRepositories>>>(key, (current) => {
      const retained = (current?.repositories ?? []).filter((candidate) => candidate.id !== repository.id)
      return {
        canManage: current?.canManage ?? repository.capabilities.manage,
        repositories: [...retained, repository].sort((left, right) =>
          `${left.owner}/${left.name}`.localeCompare(`${right.owner}/${right.name}`),
        ),
      }
    })
  }
  const prepare = useMutation({
    mutationFn: (repositoryId: string) => prepareRepository(projectId, repositoryId, crypto.randomUUID()),
    onSuccess: update,
  })
  const archive = useMutation({
    mutationFn: (repository: Repository) => archiveRepository(projectId, repository.id, repository.version, crypto.randomUUID()),
    onSuccess: update,
  })
  const [error, setError] = useState<string | null>(null)

  if (repositories.isPending) return <ListSkeleton label="Loading repositories" rows={2} />
  if (repositories.isError) {
    return (
      <ErrorState action={<Button onClick={() => void repositories.refetch()}>Try again</Button>} title="Repositories didn’t load">
        Check your connection and try again.
      </ErrorState>
    )
  }
  const canManage = repositories.data.canManage

  return (
    <div className="aw-settings-stack">
      {error && <Banner tone="danger">{error}</Banner>}
      {repositories.data.repositories.length ? (
        <RowList label="Repositories">
          {repositories.data.repositories.map((repository) => (
            <Row
              actions={
                <>
                  <RepositoryState repository={repository} />
                  {repository.capabilities.manage && !repository.archivedAt && (
                    <>
                      {repository.preparationState !== 'ready' && (
                        <Button
                          busy={prepare.isPending && prepare.variables === repository.id}
                          disabled={prepare.isPending}
                          icon="retry"
                          onClick={() => {
                            setError(null)
                            prepare.mutate(repository.id, { onError: (reason) => setError(errorMessage(reason)) })
                          }}
                          size="sm"
                        >
                          Prepare mirror
                        </Button>
                      )}
                      <Button
                        disabled={archive.isPending}
                        icon="archive"
                        onClick={() => {
                          setError(null)
                          archive.mutate(repository, { onError: (reason) => setError(errorMessage(reason)) })
                        }}
                        size="sm"
                        variant="ghost"
                      >
                        Archive repository
                      </Button>
                    </>
                  )}
                </>
              }
              heading
              key={repository.id}
              leading={
                <span className="aw-row__tile">
                  <Icon name="repo" size={16} />
                </span>
              }
              meta={
                <span className="aw-row__metaline">
                  <Icon name="branch" size={12} />
                  {repository.defaultBranch}
                </span>
              }
              title={`${repository.owner}/${repository.name}`}
            >
              {repository.preparationFailureCategory && !repository.archivedAt && (
                <p className="aw-row__note">
                  Setting up the server’s copy failed: {humanize(repository.preparationFailureCategory).toLocaleLowerCase()}. Prepare the
                  mirror again, or check the host.
                </p>
              )}
            </Row>
          ))}
        </RowList>
      ) : (
        <p className="aw-quiet">
          No repositories yet.{canManage ? (showRegistration ? ' Register one below to start runs against it.' : ' Add one from GitHub above.') : ''}
        </p>
      )}
      {canManage && showRegistration && <RepositoryRegistrationForm onCreated={update} projectId={projectId} />}
    </div>
  )
}

/** Register a repository by typing its owner, name and GitHub ID: for hosts without the GitHub connect flow. */
export function RepositoryRegistrationForm({ onCreated, projectId }: { onCreated: (repository: Repository) => void; projectId: string }) {
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
    <form aria-labelledby="register-repository-heading" className="aw-card aw-formcard aw-form" onSubmit={submit}>
      <div>
        <h3 className="aw-formcard__title" id="register-repository-heading">
          Register GitHub repository
        </h3>
        <p className="aw-formcard__desc">Add a repository from GitHub so runs in this Project can work in it.</p>
      </div>
      <div className="aw-form__grid">
        <TextField id="repository-owner" label="Owner" onChange={(event) => setOwner(event.target.value)} placeholder="Mischa-dev" value={owner} />
        <TextField id="repository-name" label="Repository" onChange={(event) => setName(event.target.value)} placeholder="agent-workspace" value={name} />
        <TextField
          hint="The number in the repository’s GitHub API record."
          id="github-repository-id"
          inputMode="numeric"
          label="GitHub repository ID"
          mono
          onChange={(event) => setGithubRepositoryId(event.target.value)}
          value={githubRepositoryId}
        />
        <TextField
          id="repository-default-branch"
          label="Default branch"
          mono
          onChange={(event) => setDefaultBranch(event.target.value)}
          value={defaultBranch}
        />
      </div>
      <Checkbox
        checked={trusted}
        label="I trust this repository. Runs execute its code on the server under the Agent Workspace service account."
        onChange={setTrusted}
      />
      {error && <Banner tone="danger">{error}</Banner>}
      <div className="aw-form__actions">
        <Button busy={registration.isPending} disabled={!valid} icon="plus" type="submit" variant="primary">
          Register repository
        </Button>
      </div>
    </form>
  )
}

function errorMessage(reason: unknown) {
  return isApiError(reason) ? reason.message : 'That didn’t work. Try again.'
}
