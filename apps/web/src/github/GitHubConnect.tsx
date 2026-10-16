import { useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { toastError } from '../app/mutations'
import { RepositoryRegistrationForm } from '../repositories/RepositorySettings'
import {
  Banner,
  Button,
  EmptyState,
  ErrorState,
  Icon,
  ListSkeleton,
  Row,
  RowList,
  SettingsSkeleton,
  StatusChip,
  TextField,
  formatShortDate,
  showToast,
} from '../ui'
import {
  addGitHubRepository,
  listAvailableGitHubRepositories,
  type GitHubAvailableRepository,
  type GitHubConnection,
} from './api'
import { availableRepositoriesKey as availableKey, useGitHubConnect } from './connect'

/** The buttons that start the connect flow, and the installations already connected. */
export function GitHubConnectPanel({ connection, projectId }: { connection: GitHubConnection; projectId: string }) {
  const connect = useGitHubConnect(projectId)
  const connected = connection.installations
  return (
    <div className="aw-card aw-profile aw-ghconnect">
      <header className="aw-profile__head">
        <span className="aw-row__tile">
          <Icon name="pull-request" size={16} />
        </span>
        <div className="aw-profile__titles">
          <h3 className="aw-profile__name">GitHub</h3>
          <p className="aw-profile__sub">
            {connected.length
              ? `Connected to ${connected.map((installation) => installation.accountLogin).join(', ')}.`
              : 'Install the GitHub App on your account or organization and pick the repositories it may use.'}
          </p>
        </div>
        {connected.length > 0 && (
          <StatusChip icon="circle-check" tone="success">
            Connected
          </StatusChip>
        )}
      </header>
      {connection.capabilities.manage ? (
        <div className="aw-card__actions aw-ghconnect__actions">
          <Button busy={connect.isPending && connect.variables === 'install'} disabled={connect.isPending} icon="external" onClick={() => connect.mutate('install')} variant="primary">
            {connected.length ? 'Connect another account' : 'Connect GitHub'}
          </Button>
          <Button busy={connect.isPending && connect.variables === 'authorize'} disabled={connect.isPending} onClick={() => connect.mutate('authorize')} variant="ghost">
            I already installed it
          </Button>
        </div>
      ) : (
        <p className="aw-quiet">Project owners and admins connect GitHub.</p>
      )}
    </div>
  )
}

/** Lists the installations the workspace connected. */
export function GitHubInstallationList({ connection }: { connection: GitHubConnection }) {
  if (!connection.installations.length) return <p className="aw-quiet">No GitHub accounts are connected yet.</p>
  return (
    <RowList label="Connected GitHub accounts">
      {connection.installations.map((installation) => (
        <Row
          key={installation.installationId}
          leading={
            <span className="aw-row__tile">
              <Icon name={installation.targetType === 'Organization' ? 'users' : 'at-sign'} size={16} />
            </span>
          }
          meta={`${installation.targetType === 'Organization' ? 'Organization' : 'Personal account'} · connected by ${installation.claimedByDisplayName} on ${formatShortDate(installation.claimedAt)}`}
          title={installation.accountLogin}
        />
      ))}
    </RowList>
  )
}

/** Shown when the host can't run the connect flow. */
export function GitHubConnectUnavailable({ connection, projectId }: { connection: GitHubConnection; projectId: string }) {
  const queryClient = useQueryClient()
  return (
    <div className="aw-settings-stack">
      <Banner role="note" tone="info" title={connection.configured ? 'Connecting from here isn’t set up.' : 'GitHub isn’t set up on this server.'}>
        {connection.configured
          ? 'To connect repositories in a few clicks, the host operator must set the GitHub App’s slug and its OAuth client ID and secret.'
          : 'A host operator must install the GitHub App’s credentials and set its slug and OAuth client before Projects can connect to GitHub.'}
      </Banner>
      {connection.capabilities.manage && (
        <details className="aw-disclosure">
          <summary>Register manually</summary>
          <RepositoryRegistrationForm
            onCreated={() => void queryClient.invalidateQueries({ queryKey: queryKeys.repositories(projectId) })}
            projectId={projectId}
          />
        </details>
      )}
    </div>
  )
}

/** Repositories the connected installations can reach, with Add for each. */
export function GitHubRepositoryPicker({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient()
  const [search, setSearch] = useState('')
  const available = useQuery({ queryKey: availableKey(projectId), queryFn: () => listAvailableGitHubRepositories(projectId) })
  const add = useMutation({
    mutationFn: (repository: GitHubAvailableRepository) =>
      addGitHubRepository(projectId, { installationId: repository.installationId, githubRepositoryId: repository.githubRepositoryId }),
    onSuccess: (_created, repository) => {
      queryClient.setQueryData<{ repositories: GitHubAvailableRepository[] }>(availableKey(projectId), (current) =>
        current
          ? {
              repositories: current.repositories.map((candidate) =>
                candidate.githubRepositoryId === repository.githubRepositoryId ? { ...candidate, added: true } : candidate,
              ),
            }
          : current,
      )
      showToast({
        tone: 'success',
        title: `${repository.fullName} added`,
        description: 'The server is making its own copy. Runs can use it once it’s ready.',
      })
    },
    onError: (error, repository) => {
      if (toastError(`Couldn’t add ${repository.fullName}`, error)) void available.refetch()
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: queryKeys.repositories(projectId) }),
  })

  const groups = useMemo(() => {
    const terms = search.trim().toLocaleLowerCase()
    const matching = (available.data?.repositories ?? []).filter(
      (repository) =>
        !terms ||
        repository.fullName.toLocaleLowerCase().includes(terms) ||
        (repository.description ?? '').toLocaleLowerCase().includes(terms),
    )
    const byAccount = new Map<string, GitHubAvailableRepository[]>()
    for (const repository of matching) {
      byAccount.set(repository.accountLogin, [...(byAccount.get(repository.accountLogin) ?? []), repository])
    }
    return [...byAccount.entries()].sort(([left], [right]) => left.localeCompare(right))
  }, [available.data, search])

  if (available.isPending) return <ListSkeleton label="Loading repositories from GitHub" rows={3} />
  if (available.isError) {
    return (
      <ErrorState action={<Button onClick={() => void available.refetch()}>Try again</Button>} title="GitHub’s repositories didn’t load">
        GitHub may be slow to answer. Try again in a moment.
      </ErrorState>
    )
  }
  if (!available.data.repositories.length) {
    return (
      <EmptyState icon="repo" title="No repositories to add">
        The GitHub App can’t reach any repositories yet. Change its repository access on GitHub, then come back.
      </EmptyState>
    )
  }

  return (
    <div className="aw-ghpicker">
      <TextField
        aria-label="Search repositories"
        onChange={(event) => setSearch(event.target.value)}
        placeholder="Search repositories"
        prefix={<Icon name="search" size={14} />}
        type="search"
        value={search}
      />
      {groups.length ? (
        groups.map(([account, repositories]) => (
          <div className="aw-ghpicker__group" key={account}>
            <h4 className="aw-ghpicker__account">{account}</h4>
            <RowList label={`Repositories in ${account}`}>
              {repositories.map((repository) => (
                <Row
                  actions={
                    repository.added ? (
                      <StatusChip icon="check" tone="success">
                        Added
                      </StatusChip>
                    ) : (
                      <Button
                        aria-label={`Add ${repository.fullName}`}
                        busy={add.isPending && add.variables?.githubRepositoryId === repository.githubRepositoryId}
                        disabled={add.isPending}
                        icon="plus"
                        onClick={() => add.mutate(repository)}
                        size="sm"
                      >
                        Add
                      </Button>
                    )
                  }
                  key={repository.githubRepositoryId}
                  leading={
                    <span className="aw-row__tile">
                      <Icon name="repo" size={16} />
                    </span>
                  }
                  meta={
                    <span className="aw-row__metaline">
                      {repository.private && (
                        <StatusChip icon="lock">Private</StatusChip>
                      )}
                      <Icon name="branch" size={12} />
                      {repository.defaultBranch}
                      {repository.description ? ` · ${repository.description}` : ''}
                    </span>
                  }
                  title={repository.name}
                />
              ))}
            </RowList>
          </div>
        ))
      ) : (
        <p className="aw-quiet" role="status">
          No repositories match “{search.trim()}”.
        </p>
      )}
    </div>
  )
}

export function GitHubConnectionLoading() {
  return <SettingsSkeleton label="Loading GitHub" sections={1} />
}
