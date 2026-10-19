import { useMemo, useState, type FormEvent } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'

import { describeError } from '../../app/mutations'
import { useProfiles } from '../../agents/hooks'
import {
  createRule,
  updateRule,
  type AutomationRule,
  type Recipients,
  type RuleAction,
  type RuleActionKind,
  type RuleDraft,
  type Trigger,
  type TriggerKind,
} from '../../automations/api'
import { getProjectConversationSidebar } from '../../conversations/api'
import { listEnvironments } from '../../launch/api'
import { Banner, Button, Checkbox, Dialog, IconButton, SelectField, TextArea, TextField, showToast } from '../../ui'
import '../../automations/automations.css'
import { WEEKDAYS } from '../../automations/ruleModel'
import { listViews } from '../../views/api'
import { workItemPriorities } from '../../work-items/constants'
import { useWorkItemWorkflow } from '../../work-items/hooks'
import { useLabels } from '../../work-items/planningHooks'
import { TimeZoneField } from '../TimeZoneField'
import { deviceTimeZone } from '../timeZones'

const MAX_ACTIONS = 5
const RUN_OUTCOMES = [
  { value: 'completed', label: 'Completed' },
  { value: 'failed', label: 'Failed' },
  { value: 'canceled', label: 'Canceled' },
  { value: 'interrupted', label: 'Interrupted' },
]

const TRIGGERS: { kind: TriggerKind; label: string }[] = [
  {
    kind: 'pull_request_checks_failed',
    label: 'Checks fail on a pull request',
  },
  { kind: 'run_finished', label: 'A run finishes' },
  { kind: 'task_created', label: 'A task is created' },
  { kind: 'task_matches', label: 'A task starts matching the view' },
  { kind: 'message_posted', label: 'Someone posts a message' },
  { kind: 'schedule', label: 'On a schedule' },
  {
    kind: 'capacity_window',
    label: 'An account for runs resets soon with capacity left',
  },
]

const ACTIONS: { kind: RuleActionKind; label: string }[] = [
  { kind: 'launch', label: 'Launch an agent' },
  { kind: 'set_field', label: 'Change a field' },
  { kind: 'notify', label: 'Notify people' },
  { kind: 'post_message', label: 'Post a message' },
]

function defaultTrigger(kind: TriggerKind): Trigger {
  switch (kind) {
    case 'run_finished':
      return { kind, outcomes: ['failed'] }
    case 'message_posted':
      return { kind, conversationId: null, contains: null }
    case 'schedule':
      return {
        kind,
        frequency: 'weekly',
        every: 1,
        weekdays: [1],
        monthDay: null,
        time: '09:00',
        timezone: deviceTimeZone(),
      }
    case 'capacity_window':
      return { kind, providerAccountId: null, maxTasks: 3 }
    default:
      return { kind }
  }
}

function defaultAction(kind: RuleActionKind): RuleAction {
  switch (kind) {
    case 'launch':
      return { kind }
    case 'set_field':
      return { kind, field: 'status', value: '' }
    case 'notify':
      return {
        kind,
        recipients: 'owner',
        message: '{{rule.name}}: {{task.key}} {{task.title}}',
      }
    case 'post_message':
      return { kind, conversationId: '', text: '' }
  }
}

/** Make or change an automation rule: a trigger, a saved view, and up to five actions. */
export function RuleDialog({
  onClose,
  onSaved,
  projectId,
  rule,
}: {
  onClose: () => void
  onSaved: () => void
  projectId: string
  rule: AutomationRule | null
}) {
  const [name, setName] = useState(rule?.name ?? '')
  const [trigger, setTrigger] = useState<Trigger>(rule?.trigger ?? defaultTrigger('pull_request_checks_failed'))
  const [viewId, setViewId] = useState(rule?.conditions.viewId ?? '')
  const [actions, setActions] = useState<RuleAction[]>(rule?.actions ?? [defaultAction('launch')])
  const [error, setError] = useState<string | null>(null)
  // A half-made rule survives a stray click outside the dialog.
  const [initial] = useState(() => JSON.stringify({ name, trigger, viewId, actions }))
  const dirty = JSON.stringify({ name, trigger, viewId, actions }) !== initial
  const views = useQuery({
    queryKey: ['project', projectId, 'views'],
    queryFn: () => listViews(projectId),
  })
  const environments = useQuery({
    queryKey: ['project', projectId, 'environments'],
    queryFn: () => listEnvironments(projectId),
  })
  const profiles = useProfiles(projectId)
  const channels = useQuery({
    queryKey: ['project', projectId, 'automations', 'channels'],
    queryFn: () => getProjectConversationSidebar(projectId),
  })
  const channelList = channels.data?.channels.filter((channel) => !channel.archivedAt) ?? []

  const save = useMutation({
    mutationFn: async () => {
      const draft: RuleDraft = {
        name: name.trim(),
        trigger,
        conditions: {
          ...(rule?.conditions.filters ? { filters: rule.conditions.filters } : {}),
          viewId: viewId || null,
        },
        actions,
      }
      return rule ? updateRule(projectId, rule, draft) : createRule(projectId, draft)
    },
    onSuccess: (saved) => {
      showToast({
        tone: 'success',
        title: rule ? 'Rule saved' : `${saved.name} is on`,
      })
      onSaved()
    },
    onError: (reason) => setError(describeError(reason).message),
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim() || !actions.length) return
    save.mutate()
  }
  const setAction = (index: number, action: RuleAction) =>
    setActions((current) => current.map((entry, at) => (at === index ? action : entry)))

  return (
    <Dialog
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={save.isPending} disabled={!name.trim() || !actions.length} type="submit" variant="primary">
            {rule ? 'Save changes' : 'Create rule'}
          </Button>
        </>
      }
      dirty={dirty}
      onClose={onClose}
      onSubmit={submit}
      open
      title={rule ? `Edit ${rule.name}` : 'New rule'}
      width={600}
    >
      <div className="aw-form aw-rule-form">
        <TextField id="rule-name" label="Name" maxLength={120} onChange={(event) => setName(event.target.value)} required value={name} />
        <fieldset className="aw-fieldset">
          <legend className="aw-field__label">When</legend>
          <SelectField
            aria-label="Trigger"
            id="rule-trigger"
            onChange={(event) => setTrigger(defaultTrigger(event.target.value as TriggerKind))}
            value={trigger.kind}
          >
            {TRIGGERS.map((entry) => (
              <option key={entry.kind} value={entry.kind}>
                {entry.label}
              </option>
            ))}
          </SelectField>
          <TriggerFields channels={channelList} onChange={setTrigger} trigger={trigger} />
        </fieldset>
        <SelectField
          hint="Only tasks in this saved view count. Left out, every task does."
          id="rule-view"
          label="Only for tasks in"
          onChange={(event) => setViewId(event.target.value)}
          value={viewId}
        >
          <option value="">Any task</option>
          {views.data?.views.map((view) => (
            <option key={view.id} value={view.id}>
              {view.name}
            </option>
          ))}
        </SelectField>
        <fieldset className="aw-fieldset">
          <legend className="aw-field__label">Then</legend>
          {actions.map((action, index) => (
            <div className="aw-rule-action" key={index}>
              <div className="aw-rule-action__head">
                <SelectField
                  aria-label={`Action ${index + 1}`}
                  id={`rule-action-${index}`}
                  onChange={(event) => setAction(index, defaultAction(event.target.value as RuleActionKind))}
                  value={action.kind}
                >
                  {ACTIONS.map((entry) => (
                    <option key={entry.kind} value={entry.kind}>
                      {entry.label}
                    </option>
                  ))}
                </SelectField>
                {actions.length > 1 && (
                  <IconButton
                    icon="trash"
                    label={`Remove action ${index + 1}`}
                    onClick={() => setActions((current) => current.filter((_, at) => at !== index))}
                    size="sm"
                  />
                )}
              </div>
              <ActionFields
                action={action}
                channels={channelList}
                environments={environments.data?.environments ?? []}
                index={index}
                profiles={(profiles.data?.profiles ?? []).filter((profile) => profile.enabled)}
                onChange={(next) => setAction(index, next)}
                projectId={projectId}
              />
            </div>
          ))}
          {actions.length < MAX_ACTIONS && (
            <Button icon="plus" onClick={() => setActions((current) => [...current, defaultAction('notify')])} size="sm" variant="ghost">
              Add an action
            </Button>
          )}
          <p className="aw-quiet">
            Text can use {'{{task.key}}'}, {'{{task.title}}'}, {'{{task.url}}'}, {'{{run.state}}'}, {'{{pr.number}}'}, {'{{pr.url}}'} and{' '}
            {'{{rule.name}}'}.
          </p>
        </fieldset>
        {error && <Banner tone="danger">{error}</Banner>}
      </div>
    </Dialog>
  )
}

type ChannelChoice = { id: string; name: string }

function TriggerFields({
  channels,
  onChange,
  trigger,
}: {
  channels: ChannelChoice[]
  onChange: (trigger: Trigger) => void
  trigger: Trigger
}) {
  switch (trigger.kind) {
    case 'run_finished':
      return (
        <div className="aw-rule-choices">
          {RUN_OUTCOMES.map((outcome) => (
            <Checkbox
              checked={trigger.outcomes?.includes(outcome.value) ?? false}
              key={outcome.value}
              label={outcome.label}
              onChange={(checked) =>
                onChange({
                  ...trigger,
                  outcomes: checked
                    ? [...(trigger.outcomes ?? []), outcome.value]
                    : (trigger.outcomes ?? []).filter((entry) => entry !== outcome.value),
                })
              }
            />
          ))}
          <p className="aw-quiet">None ticked means any outcome.</p>
        </div>
      )
    case 'message_posted':
      return (
        <>
          <SelectField
            id="rule-trigger-channel"
            label="In"
            onChange={(event) =>
              onChange({
                ...trigger,
                conversationId: event.target.value || null,
              })
            }
            value={trigger.conversationId ?? ''}
          >
            <option value="">Any conversation you can see</option>
            {channels.map((channel) => (
              <option key={channel.id} value={channel.id}>
                #{channel.name}
              </option>
            ))}
          </SelectField>
          <TextField
            hint="Left empty, any message counts. Your own messages never do."
            id="rule-trigger-contains"
            label="Containing"
            onChange={(event) => onChange({ ...trigger, contains: event.target.value || null })}
            value={trigger.contains ?? ''}
          />
        </>
      )
    case 'schedule':
      return (
        <>
          <div className="aw-rule-inline">
            <SelectField
              id="rule-schedule-frequency"
              label="Repeats"
              onChange={(event) =>
                onChange({
                  ...trigger,
                  frequency: event.target.value as 'daily' | 'weekly' | 'monthly',
                })
              }
              value={trigger.frequency}
            >
              <option value="daily">Daily</option>
              <option value="weekly">Weekly</option>
              <option value="monthly">Monthly</option>
            </SelectField>
            <TextField
              id="rule-schedule-every"
              label="Every"
              max={52}
              min={1}
              onChange={(event) =>
                onChange({
                  ...trigger,
                  every: Math.max(1, Number(event.target.value) || 1),
                })
              }
              type="number"
              value={trigger.every}
            />
            <TextField
              id="rule-schedule-time"
              label="At"
              onChange={(event) => onChange({ ...trigger, time: event.target.value })}
              type="time"
              value={trigger.time}
            />
          </div>
          {trigger.frequency === 'weekly' && (
            <div className="aw-rule-choices aw-rule-choices--row">
              {WEEKDAYS.map((day, index) => (
                <Checkbox
                  checked={trigger.weekdays?.includes(index + 1) ?? false}
                  key={day}
                  label={day}
                  onChange={(checked) =>
                    onChange({
                      ...trigger,
                      weekdays: checked
                        ? [...(trigger.weekdays ?? []), index + 1].sort()
                        : (trigger.weekdays ?? []).filter((entry) => entry !== index + 1),
                    })
                  }
                />
              ))}
            </div>
          )}
          {trigger.frequency === 'monthly' && (
            <TextField
              id="rule-schedule-day"
              label="On day"
              max={31}
              min={1}
              onChange={(event) =>
                onChange({
                  ...trigger,
                  monthDay: Number(event.target.value) || null,
                })
              }
              type="number"
              value={trigger.monthDay ?? ''}
            />
          )}
          <TimeZoneField id="rule-schedule-timezone" onChange={(timezone) => onChange({ ...trigger, timezone })} value={trigger.timezone} />
        </>
      )
    case 'capacity_window':
      return (
        <TextField
          hint="Launches use the account that resets, on tasks in the view below."
          id="rule-capacity-max"
          label="Launch at most this many tasks"
          max={10}
          min={1}
          onChange={(event) =>
            onChange({
              ...trigger,
              maxTasks: Number(event.target.value) || null,
            })
          }
          type="number"
          value={trigger.maxTasks ?? 3}
        />
      )
    case 'pull_request_checks_failed':
      return <p className="aw-quiet">On a pull request linked to a task or run in this project.</p>
    default:
      return null
  }
}

function ActionFields({
  action,
  channels,
  environments,
  index,
  onChange,
  profiles,
  projectId,
}: {
  action: RuleAction
  channels: ChannelChoice[]
  environments: { id: string; name: string; isDefault: boolean }[]
  index: number
  onChange: (action: RuleAction) => void
  profiles: { id: string; name: string; handle: string }[]
  projectId: string
}) {
  const id = (name: string) => `rule-action-${index}-${name}`
  switch (action.kind) {
    case 'launch':
      return (
        <>
          <SelectField
            hint="A project agent brings its own harness, model settings, instructions and environment."
            id={id('agent')}
            label="Agent"
            onChange={(event) => onChange({ ...action, agentProfileId: event.target.value || null })}
            value={action.agentProfileId ?? ''}
          >
            <option value="">The project’s run defaults</option>
            {profiles.map((profile) => (
              <option key={profile.id} value={profile.id}>
                {profile.name} (@{profile.handle})
              </option>
            ))}
          </SelectField>
          <SelectField
            id={id('environment')}
            label="Environment"
            onChange={(event) => onChange({ ...action, environmentId: event.target.value || null })}
            value={action.environmentId ?? ''}
          >
            <option value="">The project’s default</option>
            {environments.map((environment) => (
              <option key={environment.id} value={environment.id}>
                {environment.name}
              </option>
            ))}
          </SelectField>
          <TextArea
            hint="Left empty, the launch template’s prompt; for failing checks, one that asks the agent to fix them."
            id={id('prompt')}
            label="Prompt"
            onChange={(event) => onChange({ ...action, prompt: event.target.value || null })}
            rows={3}
            value={action.prompt ?? ''}
          />
        </>
      )
    case 'set_field':
      return (
        <div className="aw-rule-inline">
          <SelectField
            id={id('field')}
            label="Field"
            onChange={(event) =>
              onChange({
                ...action,
                field: event.target.value as typeof action.field,
                value: '',
              })
            }
            value={action.field}
          >
            <option value="status">Status</option>
            <option value="priority">Priority</option>
            <option value="assignee">Assignee</option>
            <option value="add_label">Add label</option>
            <option value="remove_label">Remove label</option>
          </SelectField>
          <FieldValue field={action.field} id={id('value')} onChange={(value) => onChange({ ...action, value })} projectId={projectId} value={action.value} />
        </div>
      )
    case 'notify':
      return (
        <>
          <SelectField
            id={id('recipients')}
            label="Who"
            onChange={(event) =>
              onChange({
                ...action,
                recipients: event.target.value as Recipients,
              })
            }
            value={action.recipients}
          >
            <option value="owner">Me</option>
            <option value="assignee">The task’s assignee</option>
            <option value="project_admins">Project admins</option>
          </SelectField>
          <TextField
            id={id('message')}
            label="Message"
            onChange={(event) => onChange({ ...action, message: event.target.value })}
            required
            value={action.message}
          />
        </>
      )
    case 'post_message':
      return (
        <>
          <SelectField
            id={id('channel')}
            label="Channel"
            onChange={(event) => onChange({ ...action, conversationId: event.target.value })}
            required
            value={action.conversationId}
          >
            <option disabled value="">
              Choose a channel
            </option>
            {channels.map((channel) => (
              <option key={channel.id} value={channel.id}>
                #{channel.name}
              </option>
            ))}
          </SelectField>
          <TextArea
            id={id('text')}
            label="Message"
            onChange={(event) => onChange({ ...action, text: event.target.value })}
            required
            rows={2}
            value={action.text}
          />
        </>
      )
  }
}

/** What a "Change a field" action sets: a status, priority or label from the project, or a person. */
function FieldValue({
  field,
  id,
  onChange,
  projectId,
  value,
}: {
  field: Extract<RuleAction, { kind: 'set_field' }>['field']
  id: string
  onChange: (value: string) => void
  projectId: string
  value: string
}) {
  const workflow = useWorkItemWorkflow(field === 'status' ? projectId : '')
  const labels = useLabels(field === 'add_label' || field === 'remove_label' ? projectId : '')
  const choices = useMemo(() => {
    if (field === 'status') return workflow.data?.statuses.map((status) => ({ value: status.name, label: status.name })) ?? null
    if (field === 'priority') return workItemPriorities.map((priority) => ({ value: priority.value, label: priority.label }))
    if (field === 'add_label' || field === 'remove_label') return labels.data?.map((label) => ({ value: label.name, label: label.name })) ?? null
    return null
  }, [field, labels.data, workflow.data])

  if (field === 'assignee' || !choices) {
    return (
      <TextField
        hint={field === 'assignee' ? 'me, none, or someone’s @handle' : undefined}
        id={id}
        label="To"
        onChange={(event) => onChange(event.target.value)}
        required
        value={value}
      />
    )
  }
  const known = choices.some((choice) => choice.value === value)
  return (
    <SelectField id={id} label="To" onChange={(event) => onChange(event.target.value)} required value={value}>
      <option disabled value="">
        {field === 'status' ? 'Choose a status' : field === 'priority' ? 'Choose a priority' : 'Choose a label'}
      </option>
      {value && !known ? <option value={value}>{value}</option> : null}
      {choices.map((choice) => (
        <option key={choice.value} value={choice.value}>
          {choice.label}
        </option>
      ))}
    </SelectField>
  )
}
