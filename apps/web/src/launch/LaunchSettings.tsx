import { useRef, useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { queryKeys } from '../api/queryKeys'
import { listHarnesses } from '../execution/api'
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
  Section,
  SelectField,
  StatusChip,
  TextArea,
  TextField,
  formatRelative,
  showToast,
} from '../ui'
import type { Environment, EnvironmentList, SettingSource } from './api'
import { useEnvironmentAction, useEnvironments, useProjectSettings, useSaveEnvironment, useSaveProjectSetting } from './hooks'
import { sourceLabel } from './launchModel'
import './launch.css'

function errorText(error: unknown, fallback: string) {
  if (isApiError(error) && error.code === 'version_conflict') return 'Someone changed this first. Close this and try again.'
  return isApiError(error) && error.message ? error.message : fallback
}

/** Project settings, Launch tab: where launched agents run, what they run with, and the prompt they get. */
export function LaunchSettings({ projectId }: { projectId: string }) {
  return (
    <>
      <Section
        description="Where launched agents work, such as a cloud environment set up with your repository. The default is picked when you press Launch."
        id="environments-heading"
        title="Environments"
      >
        <EnvironmentSettings projectId={projectId} />
      </Section>
      <Section
        description="What runs use unless a launch picks something else. Each person can still set their own, and a workspace default applies where the project sets none."
        id="run-defaults-heading"
        title="Run defaults"
      >
        <RunDefaults projectId={projectId} />
      </Section>
      <Section
        description="The prompt Launch writes from a task. Fields in double braces are filled in; a task's page shows the result before you launch."
        id="launch-template-heading"
        title="Launch prompt"
      >
        <TemplateEditor projectId={projectId} />
      </Section>
    </>
  )
}

function EnvironmentSettings({ projectId }: { projectId: string }) {
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

const REASONING = ['minimal', 'low', 'medium', 'high', 'max']
const SPEEDS = ['standard', 'fast']

function RunDefaults({ projectId }: { projectId: string }) {
  const settings = useProjectSettings(projectId)
  const harnesses = useQuery({ queryKey: queryKeys.harnesses(projectId), queryFn: () => listHarnesses(projectId) })
  const save = useSaveProjectSetting(projectId)

  if (settings.isPending || harnesses.isPending) return <ListSkeleton label="Loading run defaults" rows={3} />
  if (settings.isError || harnesses.isError) {
    return (
      <ErrorState
        action={
          <Button
            onClick={() => {
              void settings.refetch()
              void harnesses.refetch()
            }}
            size="sm"
          >
            Retry
          </Button>
        }
        title="Run defaults didn’t load."
      />
    )
  }
  const value = (key: string) => settings.data.settings.find((setting) => setting.key === key)
  const canEdit = settings.data.capabilities.manageProject
  const harness = value('run.harness')
  const chosenHarness = harnesses.data.find((candidate) => candidate.kind === harness?.value) ?? harnesses.data[0]
  const models = (chosenHarness?.catalog.models ?? []).filter((model) => !model.hidden)
  const write = (key: string, next: string, parse: (raw: string) => unknown = (raw) => raw) =>
    save.mutate(
      { key, value: next === '' ? undefined : parse(next) },
      { onError: (error) => showToast({ tone: 'danger', title: 'That default didn’t save.', description: errorText(error, 'Try again.') }) },
    )
  const own = (key: string) => (value(key)?.source === 'project' ? String(value(key)?.value ?? '') : '')
  const inherited = (key: string, fallback = 'Agent’s default') => {
    const setting = value(key)
    if (!setting || setting.source === 'project') return `Not set here (${fallback})`
    const shown = setting.value === null || setting.value === 'default' ? fallback : String(setting.value)
    return `Not set here (${shown}, ${sourceLabel(setting.source as SettingSource)})`
  }

  return (
    <div className="aw-form__grid">
      <SelectField disabled={!canEdit || save.isPending} id="run-default-harness" label="Agent" onChange={(event) => write('run.harness', event.target.value)} value={own('run.harness')}>
        <option value="">{inherited('run.harness', 'first connected')}</option>
        {harnesses.data.map((candidate) => (
          <option key={candidate.kind} value={candidate.kind}>
            {candidate.displayName}
          </option>
        ))}
      </SelectField>
      <SelectField disabled={!canEdit || save.isPending} id="run-default-model" label="Model" onChange={(event) => write('run.model', event.target.value)} value={own('run.model')}>
        <option value="">{inherited('run.model')}</option>
        {models.map((model) => (
          <option key={model.id} value={model.id}>
            {model.displayName}
          </option>
        ))}
      </SelectField>
      <SelectField disabled={!canEdit || save.isPending} id="run-default-reasoning" label="Thinking" onChange={(event) => write('run.reasoning', event.target.value)} value={own('run.reasoning')}>
        <option value="">{inherited('run.reasoning')}</option>
        {REASONING.map((level) => (
          <option key={level} value={level}>
            {level}
          </option>
        ))}
      </SelectField>
      <SelectField disabled={!canEdit || save.isPending} id="run-default-speed" label="Speed" onChange={(event) => write('run.speed', event.target.value)} value={own('run.speed')}>
        <option value="">{inherited('run.speed')}</option>
        {SPEEDS.map((speed) => (
          <option key={speed} value={speed}>
            {speed}
          </option>
        ))}
      </SelectField>
      <SelectField
        disabled={!canEdit || save.isPending}
        id="run-default-attempts"
        label="Attempts"
        onChange={(event) => write('run.attempts', event.target.value, Number)}
        value={own('run.attempts')}
      >
        <option value="">{inherited('run.attempts', '1')}</option>
        {Array.from({ length: 8 }, (_, index) => index + 1).map((count) => (
          <option key={count} value={count}>
            {count}
          </option>
        ))}
      </SelectField>
      {canEdit ? null : <p className="aw-launch__note">Project admins change these. Your own defaults are in your account settings.</p>}
    </div>
  )
}

function TemplateEditor({ projectId }: { projectId: string }) {
  const settings = useProjectSettings(projectId)
  const save = useSaveProjectSetting(projectId)
  const [draft, setDraft] = useState<string | null>(null)

  if (settings.isPending) return <ListSkeleton label="Loading the launch prompt" rows={3} />
  if (settings.isError) {
    return (
      <ErrorState
        action={
          <Button onClick={() => void settings.refetch()} size="sm">
            Retry
          </Button>
        }
        title="The launch prompt didn’t load."
      />
    )
  }
  const setting = settings.data.settings.find((candidate) => candidate.key === 'launch.template')
  const current = typeof setting?.value === 'string' ? setting.value : ''
  const canEdit = settings.data.capabilities.manageProject
  const text = draft ?? current
  const changed = draft !== null && draft !== current

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!changed) return
    save.mutate(
      { key: 'launch.template', value: draft },
      {
        onSuccess: () => {
          setDraft(null)
          showToast({ tone: 'success', title: 'The launch prompt was saved. The next launch uses it.' })
        },
      },
    )
  }
  const reset = () =>
    save.mutate(
      { key: 'launch.template', value: undefined },
      { onSuccess: () => setDraft(null) },
    )

  return (
    <form className="aw-launch-template" onSubmit={submit}>
      <TextArea
        disabled={!canEdit}
        hint={`Now using the ${sourceLabel(setting?.source ?? 'default')} prompt. Fields: {{task.key}}, {{task.title}}, {{task.description}}, {{task.criteria}}, {{task.url}}, {{task.priority}}, {{task.type}}, {{task.labels}}, {{task.pages}}, {{project.name}}, {{project.key}}, {{project.guide}}. Wrap a part in {{#if field}}…{{/if}} to keep it only when the field has a value, or {{#harness codex_cloud}}…{{/harness}} for one agent.`}
        id="launch-template"
        label="Prompt template"
        maxLength={12000}
        onChange={(event) => setDraft(event.target.value)}
        rows={14}
        spellCheck={false}
        value={text}
      />
      {save.isError ? (
        <Banner role="alert" tone="danger">
          {errorText(save.error, 'The prompt didn’t save. Try again.')}
        </Banner>
      ) : null}
      {canEdit ? (
        <div className="aw-launch-template__actions">
          <Button busy={save.isPending} disabled={!changed} type="submit" variant="primary">
            Save prompt
          </Button>
          {changed ? (
            <Button onClick={() => setDraft(null)} variant="ghost">
              Discard changes
            </Button>
          ) : null}
          {setting?.source === 'project' ? (
            <Button disabled={save.isPending} icon="undo" onClick={reset} variant="ghost">
              Use the {sourceLabel('workspace')} or built-in prompt
            </Button>
          ) : null}
        </div>
      ) : (
        <p className="aw-launch__note">Project admins change the launch prompt.</p>
      )}
    </form>
  )
}
