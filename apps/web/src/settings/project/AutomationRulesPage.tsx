import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { describeError, toastError } from '../../app/mutations'
import {
  automationKeys,
  createRule,
  deleteRule,
  listRules,
  listRuns,
  runRule,
  updateRule,
  type AutomationRule,
  type AutomationRun,
  type Recipients,
  type RuleAction,
  type RuleActionKind,
  type RuleDraft,
  type Trigger,
  type TriggerKind,
} from '../../automations/api'
import '../../automations/automations.css'
import { getProjectConversationSidebar } from '../../conversations/api'
import { listEnvironments } from '../../launch/api'
import {
  Banner,
  Button,
  Checkbox,
  Dialog,
  EmptyState,
  ErrorState,
  Icon,
  IconButton,
  ListSkeleton,
  Menu,
  PageHeader,
  Row,
  RowList,
  Section,
  SelectField,
  StatusChip,
  TextArea,
  TextField,
  formatRelative,
  showToast,
  type ChipTone,
} from '../../ui'
import { listViews } from '../../views/api'
import type { SettingsPageProps } from '../types'

const MAX_ACTIONS = 5
const RUN_OUTCOMES = ['completed', 'failed', 'canceled', 'interrupted']
const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']

const TRIGGERS: { kind: TriggerKind; label: string }[] = [
  { kind: 'pull_request_checks_failed', label: 'Checks fail on a pull request' },
  { kind: 'run_finished', label: 'A run finishes' },
  { kind: 'task_created', label: 'A task is created' },
  { kind: 'task_matches', label: 'A task starts matching the view' },
  { kind: 'message_posted', label: 'Someone posts a message' },
  { kind: 'schedule', label: 'On a schedule' },
  { kind: 'capacity_window', label: 'A provider account resets soon with capacity left' },
]

const ACTIONS: { kind: RuleActionKind; label: string }[] = [
  { kind: 'launch', label: 'Launch an agent' },
  { kind: 'set_field', label: 'Change a field' },
  { kind: 'notify', label: 'Notify people' },
  { kind: 'post_message', label: 'Post a message' },
]

const RUN_STATE: Record<AutomationRun['state'], { label: string; tone: ChipTone }> = {
  pending: { label: 'Running', tone: 'muted' },
  succeeded: { label: 'Done', tone: 'success' },
  partial: { label: 'Partly done', tone: 'warning' },
  failed: { label: 'Failed', tone: 'danger' },
  skipped: { label: 'Skipped', tone: 'muted' },
}

const localTimezone = () => {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
  } catch {
    return 'UTC'
  }
}

function defaultTrigger(kind: TriggerKind): Trigger {
  switch (kind) {
    case 'run_finished':
      return { kind, outcomes: ['failed'] }
    case 'message_posted':
      return { kind, conversationId: null, contains: null }
    case 'schedule':
      return { kind, frequency: 'weekly', every: 1, weekdays: [1], monthDay: null, time: '09:00', timezone: localTimezone() }
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
      return { kind, recipients: 'owner', message: '{{rule.name}}: {{task.key}} {{task.title}}' }
    case 'post_message':
      return { kind, conversationId: '', text: '' }
  }
}

/** One line for the rule list: "When checks fail on a pull request". */
function describeTrigger(trigger: Trigger): string {
  switch (trigger.kind) {
    case 'run_finished':
      return trigger.outcomes?.length ? `When a run ends ${trigger.outcomes.join(' or ')}` : 'When a run finishes'
    case 'message_posted':
      return trigger.contains ? `When a message mentions “${trigger.contains}”` : 'When someone posts a message'
    case 'schedule': {
      const unit = { daily: 'day', weekly: 'week', monthly: 'month' }[trigger.frequency]
      const every = trigger.every > 1 ? `Every ${trigger.every} ${unit}s` : `Every ${unit}`
      const days =
        trigger.frequency === 'weekly' && trigger.weekdays?.length
          ? ` on ${trigger.weekdays.map((day) => WEEKDAYS[day - 1]).join(', ')}`
          : trigger.frequency === 'monthly' && trigger.monthDay
            ? ` on day ${trigger.monthDay}`
            : ''
      return `${every}${days} at ${trigger.time} (${trigger.timezone})`
    }
    case 'task_created':
      return 'When a task is created'
    case 'task_matches':
      return 'When a task starts matching'
    case 'pull_request_checks_failed':
      return 'When checks fail on a pull request'
    case 'capacity_window':
      return 'When a provider account resets soon with capacity left'
  }
}

function describeAction(action: RuleAction): string {
  switch (action.kind) {
    case 'launch':
      return 'launch an agent'
    case 'set_field':
      return `set ${action.field.replace('_', ' ')} to ${action.value}`
    case 'notify':
      return { owner: 'notify me', assignee: 'notify the assignee', project_admins: 'notify project admins' }[action.recipients]
    case 'post_message':
      return 'post a message'
  }
}

/** Project › Automation rules: when something happens, do something, acting as the rule's maker. */
export default function AutomationRulesPage({ project }: SettingsPageProps) {
  const projectId = project?.id ?? ''
  const queryClient = useQueryClient()
  const rules = useQuery({ queryKey: automationKeys.rules(projectId), queryFn: () => listRules(projectId), enabled: Boolean(project) })
  const [editing, setEditing] = useState<AutomationRule | 'new' | null>(null)
  const [deleting, setDeleting] = useState<AutomationRule | null>(null)
  const [history, setHistory] = useState<AutomationRule | null>(null)
  const refresh = () => queryClient.invalidateQueries({ queryKey: automationKeys.rules(projectId) })

  const toggle = useMutation({
    mutationFn: (rule: AutomationRule) => updateRule(projectId, rule, { enabled: !rule.enabled }),
    onSuccess: (rule) => showToast({ title: rule.enabled ? `${rule.name} is on` : `${rule.name} is off` }),
    onError: (error) => toastError('The rule didn’t change', error),
    onSettled: refresh,
  })
  const run = useMutation({
    mutationFn: (rule: AutomationRule) => runRule(projectId, rule.id),
    onSuccess: (result) =>
      showToast({
        tone: result.state === 'failed' ? 'danger' : 'success',
        title: result.state === 'pending' ? 'Running it now' : `Ran: ${RUN_STATE[result.state].label.toLowerCase()}`,
        description: result.detail ?? undefined,
      }),
    onError: (error) => toastError('The rule didn’t run', error),
    onSettled: refresh,
  })
  const remove = useMutation({
    mutationFn: (rule: AutomationRule) => deleteRule(projectId, rule.id),
    onSuccess: () => {
      setDeleting(null)
      showToast({ title: 'Rule deleted' })
    },
    onError: (error) => {
      setDeleting(null)
      toastError('The rule wasn’t deleted', error)
    },
    onSettled: refresh,
  })

  if (!project) return null
  const canCreate = rules.data?.canCreate === true

  return (
    <>
      <PageHeader
        actions={
          canCreate ? (
            <Button icon="plus" onClick={() => setEditing('new')} variant="primary">
              New rule
            </Button>
          ) : null
        }
        icon="play"
        title="Automation rules"
      >
        When something happens in {project.name}, do something on its own. A rule acts as the person who made it, and turns itself
        off if they can no longer work here.
      </PageHeader>
      {rules.isPending ? (
        <ListSkeleton label="Loading rules" rows={3} />
      ) : rules.isError ? (
        <ErrorState action={<Button onClick={() => void rules.refetch()}>Try again</Button>} title="Rules didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : rules.data.rules.length ? (
        <Section id="automation-rules-heading" title="Rules">
          <RowList label="Automation rules">
            {rules.data.rules.map((rule) => (
              <Row
                actions={
                  <>
                    <Button onClick={() => setHistory(rule)} size="sm" variant="ghost">
                      History
                    </Button>
                    {rule.canManage && (
                      <Menu
                        items={[
                          ...(rule.canEdit
                            ? [
                                { label: 'Edit', icon: 'pencil' as const, onSelect: () => setEditing(rule) },
                                { label: 'Run now', icon: 'play' as const, disabled: !rule.enabled, onSelect: () => run.mutate(rule) },
                              ]
                            : []),
                          {
                            label: rule.enabled ? 'Turn off' : 'Turn on',
                            icon: rule.enabled ? ('stop' as const) : ('play' as const),
                            disabled: !rule.enabled && !rule.canEdit,
                            onSelect: () => toggle.mutate(rule),
                          },
                          { separator: true },
                          { label: 'Delete', icon: 'trash' as const, danger: true, onSelect: () => setDeleting(rule) },
                        ]}
                        label={`${rule.name} actions`}
                        trigger={<IconButton icon="more" label={`${rule.name} actions`} size="sm" />}
                      />
                    )}
                  </>
                }
                className={rule.enabled ? undefined : 'is-disabled'}
                key={rule.id}
                leading={
                  <span className="aw-row__tile">
                    <Icon name="play" size={16} />
                  </span>
                }
                meta={
                  <span className="aw-row__metaline aw-wrap">
                    <span>
                      {describeTrigger(rule.trigger)}
                      {rule.viewName ? ` in “${rule.viewName}”` : ''}, {rule.actions.map(describeAction).join(', then ')}
                    </span>
                    <span>By {rule.ownerName ?? 'someone who left'}</span>
                    <span>{rule.lastFiredAt ? `Last ran ${formatRelative(rule.lastFiredAt)}` : 'Hasn’t run yet'}</span>
                    {rule.enabled && rule.nextRunAt && <span>Next {formatRelative(rule.nextRunAt)}</span>}
                    {rule.disabledReason && <span>{rule.disabledReason}</span>}
                  </span>
                }
                title={
                  <span className="aw-row__titleline">
                    {rule.name}
                    <StatusChip tone={rule.enabled ? 'success' : rule.disabledReason ? 'danger' : 'muted'}>
                      {rule.enabled ? 'On' : 'Off'}
                    </StatusChip>
                  </span>
                }
              />
            ))}
          </RowList>
        </Section>
      ) : (
        <EmptyState
          action={
            canCreate ? (
              <Button icon="plus" onClick={() => setEditing('new')}>
                New rule
              </Button>
            ) : undefined
          }
          icon="play"
          title="No rules yet"
        >
          For example: when a pull request’s checks fail, launch an agent on it.
        </EmptyState>
      )}
      {editing && (
        <RuleDialog
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null)
            void refresh()
          }}
          projectId={projectId}
          rule={editing === 'new' ? null : editing}
        />
      )}
      {history && <HistoryDialog onClose={() => setHistory(null)} projectId={projectId} rule={history} />}
      <Dialog
        description={deleting ? `${deleting.name} stops for good, and its history goes too.` : undefined}
        footer={
          <>
            <Button onClick={() => setDeleting(null)}>Cancel</Button>
            <Button busy={remove.isPending} data-autofocus onClick={() => deleting && remove.mutate(deleting)} variant="danger">
              Delete rule
            </Button>
          </>
        }
        onClose={() => setDeleting(null)}
        open={deleting !== null}
        title="Delete this rule?"
      />
    </>
  )
}

function RuleDialog({
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
  const views = useQuery({ queryKey: ['project', projectId, 'views'], queryFn: () => listViews(projectId) })
  const environments = useQuery({ queryKey: ['project', projectId, 'environments'], queryFn: () => listEnvironments(projectId) })
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
        conditions: { ...(rule?.conditions.filters ? { filters: rule.conditions.filters } : {}), viewId: viewId || null },
        actions,
      }
      return rule ? updateRule(projectId, rule, draft) : createRule(projectId, draft)
    },
    onSuccess: (saved) => {
      showToast({ tone: 'success', title: rule ? 'Rule saved' : `${saved.name} is on` })
      onSaved()
    },
    onError: (reason) => setError(describeError(reason).message),
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim() || !actions.length) return
    save.mutate()
  }
  const setAction = (index: number, action: RuleAction) => setActions((current) => current.map((entry, at) => (at === index ? action : entry)))

  return (
    <Dialog
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={save.isPending} disabled={!name.trim() || !actions.length} type="submit" variant="primary">
            {rule ? 'Save' : 'Create rule'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title={rule ? `Edit ${rule.name}` : 'New rule'}
      width={600}
    >
      <div className="aw-form">
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
                onChange={(next) => setAction(index, next)}
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

function TriggerFields({ channels, onChange, trigger }: { channels: ChannelChoice[]; onChange: (trigger: Trigger) => void; trigger: Trigger }) {
  switch (trigger.kind) {
    case 'run_finished':
      return (
        <div className="aw-rule-choices">
          {RUN_OUTCOMES.map((outcome) => (
            <Checkbox
              checked={trigger.outcomes?.includes(outcome) ?? false}
              key={outcome}
              label={outcome[0].toUpperCase() + outcome.slice(1)}
              onChange={(checked) =>
                onChange({
                  ...trigger,
                  outcomes: checked ? [...(trigger.outcomes ?? []), outcome] : (trigger.outcomes ?? []).filter((entry) => entry !== outcome),
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
            onChange={(event) => onChange({ ...trigger, conversationId: event.target.value || null })}
            value={trigger.conversationId ?? ''}
          >
            <option value="">Any conversation I can see</option>
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
              onChange={(event) => onChange({ ...trigger, frequency: event.target.value as 'daily' | 'weekly' | 'monthly' })}
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
              onChange={(event) => onChange({ ...trigger, every: Math.max(1, Number(event.target.value) || 1) })}
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
              onChange={(event) => onChange({ ...trigger, monthDay: Number(event.target.value) || null })}
              type="number"
              value={trigger.monthDay ?? ''}
            />
          )}
          <TextField
            id="rule-schedule-timezone"
            label="Time zone"
            onChange={(event) => onChange({ ...trigger, timezone: event.target.value })}
            value={trigger.timezone}
          />
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
          onChange={(event) => onChange({ ...trigger, maxTasks: Number(event.target.value) || null })}
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
}: {
  action: RuleAction
  channels: ChannelChoice[]
  environments: { id: string; name: string; isDefault: boolean }[]
  index: number
  onChange: (action: RuleAction) => void
}) {
  const id = (name: string) => `rule-action-${index}-${name}`
  switch (action.kind) {
    case 'launch':
      return (
        <>
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
            onChange={(event) => onChange({ ...action, field: event.target.value as typeof action.field })}
            value={action.field}
          >
            <option value="status">Status</option>
            <option value="priority">Priority</option>
            <option value="assignee">Assignee</option>
            <option value="add_label">Add label</option>
            <option value="remove_label">Remove label</option>
          </SelectField>
          <TextField
            hint={action.field === 'assignee' ? 'me, none, or a handle' : action.field === 'status' ? 'A status name' : undefined}
            id={id('value')}
            label="To"
            onChange={(event) => onChange({ ...action, value: event.target.value })}
            required
            value={action.value}
          />
        </div>
      )
    case 'notify':
      return (
        <>
          <SelectField
            id={id('recipients')}
            label="Who"
            onChange={(event) => onChange({ ...action, recipients: event.target.value as Recipients })}
            value={action.recipients}
          >
            <option value="owner">Me</option>
            <option value="assignee">The task’s assignee</option>
            <option value="project_admins">Project admins</option>
          </SelectField>
          <TextField id={id('message')} label="Message" onChange={(event) => onChange({ ...action, message: event.target.value })} required value={action.message} />
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
          <TextArea id={id('text')} label="Message" onChange={(event) => onChange({ ...action, text: event.target.value })} required rows={2} value={action.text} />
        </>
      )
  }
}

function HistoryDialog({ onClose, projectId, rule }: { onClose: () => void; projectId: string; rule: AutomationRule }) {
  const runs = useQuery({ queryKey: automationKeys.runs(projectId, rule.id), queryFn: () => listRuns(projectId, rule.id) })
  return (
    <Dialog footer={<Button onClick={onClose}>Done</Button>} onClose={onClose} open title={`History of ${rule.name}`} width={560}>
      {runs.isPending ? (
        <ListSkeleton label="Loading history" rows={3} />
      ) : runs.isError ? (
        <ErrorState action={<Button onClick={() => void runs.refetch()}>Try again</Button>} title="History didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : runs.data.runs.length ? (
        <RowList label="Runs">
          {runs.data.runs.map((entry) => (
            <Row
              key={entry.id}
              meta={
                <span className="aw-row__metaline aw-wrap">
                  <span>{formatRelative(entry.createdAt)}</span>
                  {entry.detail && <span>{entry.detail}</span>}
                  {entry.results.map((result, index) => (
                    <span key={index}>
                      {result.ok ? '✓' : '✕'} {result.detail}
                    </span>
                  ))}
                </span>
              }
              title={
                <span className="aw-row__titleline">
                  {entry.triggerSummary}
                  <StatusChip tone={RUN_STATE[entry.state].tone}>{RUN_STATE[entry.state].label}</StatusChip>
                </span>
              }
            />
          ))}
        </RowList>
      ) : (
        <p className="aw-quiet">It hasn’t run yet.</p>
      )}
    </Dialog>
  )
}
