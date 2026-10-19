import { useRef, useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { listRepositories } from '../repositories/api'
import {
  Banner,
  Button,
  Checkbox,
  Dialog,
  EmptyState,
  ErrorState,
  Icon,
  ListSkeleton,
  Menu,
  IconButton,
  Row,
  RowList,
  SelectField,
  StatusChip,
  TextArea,
  TextField,
  formatRelative,
  showToast,
} from '../ui'
import type { Environment, EnvironmentList } from './api'
import { useEnvironmentAction, useEnvironments, useSaveEnvironment } from './hooks'
import './launch.css'

function errorText(error: unknown, fallback: string) {
  if (isApiError(error) && error.code === 'version_conflict') return 'Someone changed this first. Close this and try again.'
  return isApiError(error) && error.message ? error.message : fallback
}

/** A project's environments: where launched agents work, with the default marked. */
export function EnvironmentSettings({ projectId }: { projectId: string }) {
  const environments = useEnvironments(projectId)
  const action = useEnvironmentAction(projectId)
  const [editing, setEditing] = useState<Environment | 'new' | null>(null)
  const addButton = useRef<HTMLButtonElement>(null)

  if (environments.isPending) return <ListSkeleton label="Loading environments" rows={2} />
  if (environments.isError) {
    return (
      <ErrorState
        action={
          <Button onClick={() => void environments.refetch()} size="sm">
            Retry
          </Button>
        }
        title="Environments didn’t load."
      />
    )
  }
  const list = environments.data
  const run = (environment: Environment, kind: 'archive' | 'default' | 'check') =>
    action.mutate(
      { environmentId: environment.id, action: kind },
      {
        onSuccess: (result) => {
          if (kind === 'check' && result) {
            showToast({ tone: result.ok ? 'success' : 'danger', title: result.ok ? `${environment.name} is ready.` : `${environment.name} didn’t pass.`, description: result.message })
          } else if (kind === 'archive') {
            showToast({ title: `${environment.name} was archived. Queued launches there won’t start.` })
          }
        },
        onError: (error) => showToast({ tone: 'danger', title: 'That didn’t work.', description: errorText(error, 'Try again.') }),
      },
    )

  return (
    <>
      {list.environments.length ? (
        <RowList label="Environments">
          {list.environments.map((environment) => (
            <Row
              actions={
                list.canManage ? (
                  <Menu
                    items={[
                      { label: 'Edit', icon: 'pencil', onSelect: () => setEditing(environment) },
                      { label: 'Check it', icon: 'check', onSelect: () => run(environment, 'check') },
                      ...(environment.isDefault ? [] : [{ label: 'Make default', icon: 'star' as const, onSelect: () => run(environment, 'default') }]),
                      { separator: true as const },
                      { label: 'Archive', icon: 'archive', danger: true, onSelect: () => run(environment, 'archive') },
                    ]}
                    label={`${environment.name} actions`}
                    trigger={<IconButton icon="more" label={`${environment.name} actions`} size="sm" />}
                  />
                ) : undefined
              }
              key={environment.id}
              leading={<Icon name={environment.runsOn === 'cloud' ? 'external' : 'terminal'} />}
              meta={
                <>
                  {[environment.harnessName, environment.providerEnvironmentId, environment.branch].filter(Boolean).join(' · ')}{' '}
                  {environment.lastCheckedAt ? (
                    <span className="aw-environment__check">
                      · {environment.lastCheckOk ? 'Checked' : 'Check failed'} {formatRelative(environment.lastCheckedAt)}
                    </span>
                  ) : null}
                </>
              }
              title={
                <>
                  {environment.name}{' '}
                  {environment.isDefault ? <StatusChip tone="success">Default</StatusChip> : null}
                  {environment.lastCheckOk === false ? <StatusChip icon="circle-alert" tone="danger">Needs attention</StatusChip> : null}
                </>
              }
            >
              {environment.lastCheckOk === false && environment.lastCheckMessage ? (
                <p className="aw-launches__reason">{environment.lastCheckMessage}</p>
              ) : null}
            </Row>
          ))}
        </RowList>
      ) : (
        <EmptyState icon="external" title="No environments yet">
          Without one, launches run on this server with the project’s repository.
        </EmptyState>
      )}
      {list.canManage ? (
        <div>
          <Button icon="plus" onClick={() => setEditing('new')} ref={addButton}>
            Add environment
          </Button>
        </div>
      ) : (
        <p className="aw-launch__note">Project admins add and change environments.</p>
      )}
      {editing ? (
        <EnvironmentDialog
          environment={editing === 'new' ? null : editing}
          list={list}
          onClose={() => setEditing(null)}
          projectId={projectId}
          returnFocus={addButton}
        />
      ) : null}
    </>
  )
}

function EnvironmentDialog({
  environment,
  list,
  onClose,
  projectId,
  returnFocus,
}: {
  environment: Environment | null
  list: EnvironmentList
  onClose: () => void
  projectId: string
  returnFocus: React.RefObject<HTMLButtonElement | null>
}) {
  const save = useSaveEnvironment(projectId)
  const repositories = useQuery({ queryKey: queryKeys.repositories(projectId), queryFn: () => listRepositories(projectId) })
  const [name, setName] = useState(environment?.name ?? '')
  const [harness, setHarness] = useState(environment?.harness ?? list.harnesses.find((candidate) => candidate.runsOn === 'cloud')?.kind ?? list.harnesses[0]?.kind ?? '')
  const [providerEnvironmentId, setProviderEnvironmentId] = useState(environment?.providerEnvironmentId ?? '')
  const [repositoryId, setRepositoryId] = useState(environment?.repositoryId ?? '')
  const [branch, setBranch] = useState(environment?.branch ?? '')
  const [setupNotes, setSetupNotes] = useState(environment?.setupNotes ?? '')
  const [makeDefault, setMakeDefault] = useState(list.environments.length === 0)
  const choice = list.harnesses.find((candidate) => candidate.kind === harness)
  const needsId = Boolean(choice?.needsProviderEnvironment)
  const usable = (repositories.data?.repositories ?? []).filter((repository) => !repository.archivedAt)

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim() || !harness || (needsId && !providerEnvironmentId.trim())) return
    save.mutate(
      {
        environmentId: environment?.id,
        input: {
          name: name.trim(),
          harness,
          providerEnvironmentId: providerEnvironmentId.trim() || null,
          repositoryId: repositoryId || null,
          branch: branch.trim() || null,
          setupNotes,
          ...(environment ? { expectedVersion: environment.version } : { makeDefault }),
        },
      },
      {
        onSuccess: (saved) => {
          showToast({ tone: 'success', title: environment ? `${saved.name} was saved.` : `${saved.name} was added.` })
          onClose()
        },
      },
    )
  }

  return (
    <Dialog
      description="An environment is somewhere an agent can work. For a cloud agent it names the provider’s environment, which has your repository and setup."
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={save.isPending} disabled={!name.trim() || !harness || (needsId && !providerEnvironmentId.trim())} type="submit" variant="primary">
            {environment ? 'Save' : 'Add environment'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      returnFocus={returnFocus}
      title={environment ? `Edit ${environment.name}` : 'Add an environment'}
      width={560}
    >
      <div className="aw-form__grid">
        <TextField data-autofocus id="environment-name" label="Name" maxLength={80} onChange={(event) => setName(event.target.value)} required value={name} />
        <SelectField disabled={Boolean(environment)} id="environment-harness" label="Agent" onChange={(event) => setHarness(event.target.value)} value={harness}>
          {list.harnesses.map((candidate) => (
            <option key={candidate.kind} value={candidate.kind}>
              {candidate.displayName}
            </option>
          ))}
        </SelectField>
        {needsId || providerEnvironmentId ? (
          <TextField
            hint={`The environment’s ID in ${choice?.displayName ?? 'the provider'}.`}
            id="environment-provider-id"
            label="Provider environment"
            mono
            onChange={(event) => setProviderEnvironmentId(event.target.value)}
            required={needsId}
            spellCheck={false}
            value={providerEnvironmentId}
          />
        ) : null}
        <SelectField id="environment-repository" label="Repository" onChange={(event) => setRepositoryId(event.target.value)} value={repositoryId}>
          <option value="">The project’s repository</option>
          {usable.map((repository) => (
            <option key={repository.id} value={repository.id}>
              {repository.owner}/{repository.name}
            </option>
          ))}
        </SelectField>
        <TextField hint="Left empty, the repository’s default branch." id="environment-branch" label="Branch" mono onChange={(event) => setBranch(event.target.value)} spellCheck={false} value={branch} />
      </div>
      <TextArea hint="For people: what this environment has installed and anything to know." id="environment-notes" label="Setup notes" maxLength={4000} onChange={(event) => setSetupNotes(event.target.value)} rows={3} value={setupNotes} />
      {environment ? null : <Checkbox checked={makeDefault} label="Make it the project’s default" onChange={setMakeDefault} />}
      {save.isError ? (
        <Banner role="alert" tone="danger">
          {errorText(save.error, 'The environment didn’t save. Try again.')}
        </Banner>
      ) : null}
    </Dialog>
  )
}
