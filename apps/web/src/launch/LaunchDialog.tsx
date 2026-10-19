import { useState, type FormEvent, type RefObject } from 'react'
import { Link } from 'react-router-dom'

import { useProfiles } from '../agents/hooks'
import { isApiError } from '../api/errors'
import { humanize } from '../projects/format'
import { accountSettingsHref, projectSettingsHref } from '../settings/paths'
import { Banner, Button, Dialog, SelectField, SettingsSkeleton, TextArea, TextField, countLabel, describeError, describeLoadError } from '../ui'
import type { HarnessPlan, Launch, LaunchInput, LaunchPlan, LaunchTask, SettingSource, StartWhen } from './api'
import { useCreateLaunches, useLaunchPlan } from './hooks'
import { MAX_TASKS_PER_LAUNCH, sourceLabel } from './launchModel'
// Opened from the task list too, where nothing else has loaded these.
import './launch.css'

type Choice = { environmentId?: string; harness?: string }

/**
 * Everything a launch can change before it starts: where it runs, on whose account, which model, when it starts and
 * the prompt. Opens on the project's defaults, so pressing Launch straight away does what the one-click button does.
 */
export function LaunchDialog({
  onClose,
  onLaunched,
  projectId,
  returnFocus,
  tasks,
}: {
  onClose: () => void
  onLaunched: (launches: Launch[]) => void
  projectId: string
  returnFocus?: RefObject<HTMLElement | null>
  tasks: LaunchTask[]
}) {
  const [choice, setChoice] = useState<Choice>({})
  const first = tasks[0]
  const plan = useLaunchPlan(projectId, first?.id ?? '', choice)
  const create = useCreateLaunches(projectId)
  const profiles = useProfiles(projectId)
  const [profileId, setProfileId] = useState('')
  const [idempotencyKey] = useState(() => crypto.randomUUID())
  const [accountId, setAccountId] = useState('')
  const [model, setModel] = useState('')
  const [reasoning, setReasoning] = useState('')
  const [speed, setSpeed] = useState('')
  const [attempts, setAttempts] = useState('')
  const [startWhen, setStartWhen] = useState<StartWhen>('now')
  const [startAt, setStartAt] = useState('')
  const [prompt, setPrompt] = useState<string | null>(null)
  const single = tasks.length === 1
  const tooMany = tasks.length > MAX_TASKS_PER_LAUNCH
  const data = plan.data

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!data || create.isPending || tooMany) return
    const harness = data.harness ?? undefined
    // An environment only applies when it runs the harness the dialog shows; otherwise the harness is the choice.
    const environment = data.environments.find((candidate) => candidate.id === data.environmentId)
    const environmentFits = Boolean(environment && environment.harness === data.harness)
    const input: LaunchInput = {
      workItemIds: tasks.map((task) => task.id),
      environmentId: environmentFits ? environment?.id : undefined,
      agentProfileId: profileId || undefined,
      overrides: {
        ...(harness && !environmentFits ? { harness } : {}),
        ...(model ? { model } : {}),
        ...(reasoning ? { reasoning } : {}),
        ...(speed ? { speed } : {}),
        ...(attempts ? { attempts: Number(attempts) } : {}),
      },
      providerAccountId: runner ? undefined : accountId || undefined,
      prompt: single && prompt !== null && prompt.trim() !== data.prompt.trim() ? prompt : undefined,
      startWhen,
      startAt: startWhen === 'at' && startAt ? new Date(startAt).toISOString() : undefined,
      dialog: true,
    }
    if (!harness && !input.overrides?.harness) return
    create.mutate({ input, idempotencyKey }, { onSuccess: (result) => onLaunched(result.launches) })
  }

  const harnessPlan = data?.harnesses.find((candidate) => candidate.kind === data.harness)
  const models = harnessPlan?.models ?? []
  const chosenModel = models.find((candidate) => candidate.id === (model || String(data?.settings['run.model']?.value ?? '')))
    ?? models.find((candidate) => candidate.isDefault)
  const accounts = (data?.accounts ?? []).filter((account) => account.harness === data?.harness)
  const runner = harnessPlan?.connectedRunner === true
  const agents = (profiles.data?.profiles ?? []).filter((profile) => profile.enabled)
  const pickAgent = (id: string) => {
    setProfileId(id)
    setModel('')
    setReasoning('')
    setSpeed('')
    setAccountId('')
    const profile = agents.find((candidate) => candidate.id === id)
    setChoice(profile ? { environmentId: profile.environmentId ?? undefined, harness: profile.harness ?? undefined } : {})
  }
  const blocked = (data?.blockers.length ?? 0) > 0
  const scheduledWithoutTime = startWhen === 'at' && !startAt
  const queueing = !single || startWhen !== 'now'
  const title = single && first ? `Launch ${first.key}` : `Launch ${countLabel(tasks.length, 'task')}`
  // Something chosen or typed: a stray click outside shouldn't throw it away.
  const dirty = prompt !== null || Boolean(profileId || accountId || model || reasoning || speed || attempts || startAt || choice.environmentId || choice.harness) || startWhen !== 'now'

  return (
    <Dialog
      description={
        single && first
          ? `${first.title}. An agent works on it in the task’s thread, with the prompt below.`
          : 'Each task gets its own thread. They start as accounts have room, unblocked tasks first.'
      }
      footer={
        <>
          <Button onClick={onClose} variant="secondary">
            Cancel
          </Button>
          <Button
            busy={create.isPending}
            disabled={!data || !data.harness || (!runner && accounts.length === 0) || scheduledWithoutTime || tooMany}
            icon={queueing ? 'clock' : 'play'}
            type="submit"
            variant="primary"
          >
            {queueing ? 'Queue' : 'Launch'}
          </Button>
        </>
      }
      dirty={dirty}
      onClose={onClose}
      onSubmit={submit}
      open
      returnFocus={returnFocus}
      title={title}
      width={620}
    >
      {tooMany ? (
        <Banner className="aw-launch__banner" tone="warning">Launch takes up to {MAX_TASKS_PER_LAUNCH} tasks at a time. Select fewer and try again.</Banner>
      ) : null}
      {plan.isPending ? <SettingsSkeleton label="Loading launch options" /> : null}
      {plan.isError ? (
        <Banner className="aw-launch__banner" onRetry={() => void plan.refetch()} title="The launch options didn’t load" tone="danger">
          {isApiError(plan.error) && plan.error.message ? plan.error.message : describeLoadError(plan.error)}
        </Banner>
      ) : null}
      {data ? (
        <>
          {single && blocked ? (
            <Banner className="aw-launch__banner" icon="triangle-alert" tone="warning" title="This task is waiting on other work">
              Open blockers: {data.blockers.map((blocker) => blocker.key).join(', ')}. You can still launch it now, or start it
              when they finish.
            </Banner>
          ) : null}
          <div className="aw-form__grid">
            {agents.length > 0 ? (
              <SelectField
                hint={agents.find((profile) => profile.id === profileId)?.description ?? undefined}
                id="launch-agent"
                label="Agent"
                onChange={(event) => pickAgent(event.target.value)}
                value={profileId}
              >
                <option value="">The project’s default agent</option>
                {agents.map((profile) => (
                  <option key={profile.id} value={profile.id}>
                    {profile.name} (@{profile.handle})
                  </option>
                ))}
              </SelectField>
            ) : null}
            <SelectField
              data-autofocus
              hint={environmentHint(data, projectId)}
              id="launch-environment"
              label="Environment"
              onChange={(event) => setChoice({ environmentId: event.target.value || undefined, harness: choice.harness })}
              value={data.environmentId ?? ''}
            >
              <option value="">No environment</option>
              {data.environments.map((environment) => (
                <option key={environment.id} value={environment.id}>
                  {environment.name} · {environment.harnessName}
                  {environment.isDefault ? ' (default)' : ''}
                </option>
              ))}
            </SelectField>
            <SelectField
              disabled={Boolean(data.environmentId)}
              hint={data.environmentId ? 'Set by the environment.' : runner ? runnerHint(harnessPlan) : undefined}
              id="launch-harness"
              label="Runs on"
              onChange={(event) => {
                setModel('')
                setReasoning('')
                setSpeed('')
                setAccountId('')
                setChoice({ environmentId: data.environmentId ?? undefined, harness: event.target.value || undefined })
              }}
              value={data.harness ?? ''}
            >
              {data.harness ? null : <option value="">Choose where it runs</option>}
              {data.harnesses.map((harness) => (
                <option key={harness.kind} value={harness.kind}>
                  {harness.displayName}
                  {harness.connectedRunner ? ` (hands off to ${harness.runnerName ?? 'its runner'})` : ''}
                </option>
              ))}
            </SelectField>
            {runner ? null : (
              <SelectField
                hint={
                  accounts.length === 0 ? (
                    // One span, so the note's flex layout keeps the sentence and its link on one line.
                    <span>
                      Connect an account for {harnessPlan?.displayName ?? 'it'} first, in <Link to={accountSettingsHref('provider-accounts')}>Provider accounts</Link>.
                    </span>
                  ) : undefined
                }
                id="launch-account"
                label="Account"
                onChange={(event) => setAccountId(event.target.value)}
                value={accountId}
              >
                <option value="">{accounts.length ? 'The one with room' : 'No account'}</option>
                {accounts.map((account) => (
                  <option key={account.id} value={account.id}>
                    {account.name} · {account.running} of {account.maxConcurrentRuns} busy
                  </option>
                ))}
              </SelectField>
            )}
            <SelectField id="launch-model" label="Model" onChange={(event) => setModel(event.target.value)} value={model}>
              <option value="">{defaultLabel(data.settings['run.model']?.value, data.settings['run.model']?.source, models)}</option>
              {models.map((candidate) => (
                <option key={candidate.id} value={candidate.id}>
                  {candidate.displayName}
                </option>
              ))}
            </SelectField>
            {chosenModel && chosenModel.reasoningLevels.length > 0 ? (
              <SelectField id="launch-reasoning" label="Thinking" onChange={(event) => setReasoning(event.target.value)} value={reasoning}>
                <option value="">{defaultLabel(data.settings['run.reasoning']?.value, data.settings['run.reasoning']?.source)}</option>
                {chosenModel.reasoningLevels.map((level) => (
                  <option key={level} value={level}>
                    {humanize(level)}
                  </option>
                ))}
              </SelectField>
            ) : null}
            {chosenModel && chosenModel.speeds.length > 0 ? (
              <SelectField id="launch-speed" label="Speed" onChange={(event) => setSpeed(event.target.value)} value={speed}>
                <option value="">{defaultLabel(data.settings['run.speed']?.value, data.settings['run.speed']?.source)}</option>
                {chosenModel.speeds.map((level) => (
                  <option key={level} value={level}>
                    {humanize(level)}
                  </option>
                ))}
              </SelectField>
            ) : null}
            {harnessPlan && harnessPlan.maxAttempts > 1 ? (
              <SelectField id="launch-attempts" label="Attempts" onChange={(event) => setAttempts(event.target.value)} value={attempts}>
                <option value="">{defaultLabel(data.settings['run.attempts']?.value, data.settings['run.attempts']?.source)}</option>
                {Array.from({ length: harnessPlan.maxAttempts }, (_, index) => index + 1).map((count) => (
                  <option key={count} value={count}>
                    {count}
                  </option>
                ))}
              </SelectField>
            ) : null}
            <SelectField id="launch-start" label="Start" onChange={(event) => setStartWhen(event.target.value as StartWhen)} value={startWhen}>
              <option value="now">{single ? 'Now' : 'As accounts have room'}</option>
              <option value="at">At a time</option>
              <option value="window_reset">When the usage window resets</option>
              <option value="blockers_done">When blockers finish</option>
            </SelectField>
            {startWhen === 'at' ? (
              <TextField id="launch-start-at" label="Start at" onChange={(event) => setStartAt(event.target.value)} required type="datetime-local" value={startAt} />
            ) : null}
          </div>
          {single ? (
            <TextArea
              hint={
                prompt === null
                  ? `Built from the ${sourceLabel(data.templateSource)} launch template. Edit it for this launch only.`
                  : 'Edited for this launch only.'
              }
              id="launch-prompt"
              label="Prompt"
              maxLength={19000}
              onChange={(event) => setPrompt(event.target.value)}
              rows={10}
              value={prompt ?? data.prompt}
            />
          ) : (
            <p className="aw-launch__note">Each task’s prompt is built from the launch template when it starts.</p>
          )}
        </>
      ) : null}
      {create.isError ? (
        <Banner className="aw-launch__banner" role="alert" tone="danger">
          {isApiError(create.error) && create.error.message ? create.error.message : `The launch didn’t go through. ${describeError(create.error).message}`}
        </Banner>
      ) : null}
    </Dialog>
  )
}

function runnerHint(harness: HarnessPlan | undefined) {
  return `Hands off to ${harness?.runnerName ?? 'its runner'}, which works with its own access. No account needed.`
}

function environmentHint(plan: LaunchPlan, projectId: string) {
  const chosen = plan.environments.find((environment) => environment.id === plan.environmentId)
  if (chosen?.lastCheckOk === false) return chosen.lastCheckMessage ?? 'The last check of this environment failed.'
  if (plan.environments.length === 0) {
    return (
      <span>
        Add one in <Link to={projectSettingsHref(projectId, 'environments')}>Environments and defaults</Link>.
      </span>
    )
  }
  return undefined
}

function defaultLabel(value: unknown, source: SettingSource | undefined, models: { id: string; displayName: string }[] = []) {
  if (value === null || value === undefined || value === '' || value === 'default') return 'Agent’s default'
  const shown = models.find((candidate) => candidate.id === value)?.displayName ?? humanize(String(value))
  return `${shown} (${sourceLabel(source ?? 'default')} default)`
}
