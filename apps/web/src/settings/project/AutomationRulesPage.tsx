import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { toastError } from '../../app/mutations'
import {
  automationKeys,
  deleteRule,
  listRules,
  listRuns,
  runRule,
  updateRule,
  type AutomationRule,
  type AutomationRun,
  type RuleAction,
  type Trigger,
} from '../../automations/api'
import '../../automations/automations.css'
import { WEEKDAYS } from '../../automations/ruleModel'
import { GitHubAutomationSettings } from '../../github/GitHubIntegrationSettings'
import { priorityLabel } from '../../work-items/constants'
import { useWorkItemWorkflow } from '../../work-items/hooks'
import type { WorkItemPriority } from '../../work-items/types'
import {
  Button,
  ConfirmDialog,
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
  StatusChip,
  formatFullDate,
  formatRelative,
  showToast,
  type ChipTone,
} from '../../ui'
import type { SettingsPageProps } from '../types'
import { RuleDialog } from './AutomationRuleDialog'

const RUN_STATE: Record<AutomationRun['state'], { label: string; tone: ChipTone }> = {
  pending: { label: 'Running', tone: 'muted' },
  succeeded: { label: 'Done', tone: 'success' },
  partial: { label: 'Partly done', tone: 'warning' },
  failed: { label: 'Failed', tone: 'danger' },
  skipped: { label: 'Skipped', tone: 'muted' },
}

function describeTrigger(trigger: Trigger): string {
  switch (trigger.kind) {
    case 'run_finished':
      return trigger.outcomes?.length ? `When a run ${joinWithOr(trigger.outcomes.map((outcome) => RUN_OUTCOME_VERBS[outcome] ?? outcome))}` : 'When a run finishes'
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
      return 'When an account for runs resets soon with capacity left'
    case 'record_created':
      return 'When a record is added'
    case 'record_changed':
      return 'When a record changes'
  }
}

const RUN_OUTCOME_VERBS: Record<string, string> = {
  completed: 'completes',
  failed: 'fails',
  canceled: 'is canceled',
  interrupted: 'is interrupted',
}

/** "a, b or c". */
function joinWithOr(parts: string[]) {
  return parts.length > 1 ? `${parts.slice(0, -1).join(', ')} or ${parts[parts.length - 1]}` : (parts[0] ?? '')
}

function describeSetField(field: Extract<RuleAction, { kind: 'set_field' }>['field'], value: string) {
  switch (field) {
    case 'status':
      return `move the task to ${value}`
    case 'priority':
      return `set its priority to ${priorityLabel(value as WorkItemPriority).toLocaleLowerCase()}`
    case 'assignee':
      return value === 'none' ? 'unassign it' : `assign it to ${value === 'me' ? 'me' : value}`
    case 'add_label':
      return `add the label ${value}`
    case 'remove_label':
      return `remove the label ${value}`
  }
}

function describeAction(action: RuleAction): string {
  switch (action.kind) {
    case 'launch':
      return 'launch an agent'
    case 'set_field':
      return describeSetField(action.field, action.value)
    case 'notify':
      return {
        owner: 'notify me',
        assignee: 'notify the assignee',
        project_admins: 'notify project admins',
      }[action.recipients]
    case 'post_message':
      return 'post a message'
    case 'run_operation':
      return `run ${action.operation}`
  }
}

/**
 * Project › Automation: rules (when something happens, do something, acting as
 * the rule's maker) and what pull requests do to their linked tasks.
 */
export default function AutomationRulesPage({ project }: SettingsPageProps) {
  const projectId = project?.id ?? ''
  const queryClient = useQueryClient()
  const manage = project?.capabilities.manage === true
  const workflow = useWorkItemWorkflow(manage ? projectId : '')
  const rules = useQuery({
    queryKey: automationKeys.rules(projectId),
    queryFn: () => listRules(projectId),
    enabled: Boolean(project),
  })
  const [editing, setEditing] = useState<AutomationRule | 'new' | null>(null)
  const [deleting, setDeleting] = useState<AutomationRule | null>(null)
  const [history, setHistory] = useState<AutomationRule | null>(null)
  const refresh = () =>
    queryClient.invalidateQueries({
      queryKey: automationKeys.rules(projectId),
    })

  const toggle = useMutation({
    mutationFn: (rule: AutomationRule) => updateRule(projectId, rule, { enabled: !rule.enabled }),
    onSuccess: (rule) =>
      showToast({
        title: rule.enabled ? `${rule.name} is on` : `${rule.name} is off`,
      }),
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
        icon="checklist"
        title="Automation"
      >
        When something happens in {project.name}, do something on its own. A rule acts as the person who made it, and turns itself off if
        they can no longer work here.
      </PageHeader>
      {manage && (
        <Section
          description="Keep linked tasks in step with their pull requests on GitHub."
          id="pull-request-automation-heading"
          title="Pull requests"
        >
          <GitHubAutomationSettings projectId={projectId} statuses={workflow.data?.statuses} />
        </Section>
      )}
      {rules.isPending ? (
        <ListSkeleton label="Loading rules" rows={3} />
      ) : rules.isError ? (
        <ErrorState error={rules.error} onRetry={() => void rules.refetch()} title="Rules didn’t load" />
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
                                {
                                  label: 'Edit',
                                  icon: 'pencil' as const,
                                  onSelect: () => setEditing(rule),
                                },
                                {
                                  label: 'Run now',
                                  icon: 'play' as const,
                                  disabled: !rule.enabled,
                                  onSelect: () => run.mutate(rule),
                                },
                              ]
                            : []),
                          {
                            label: rule.enabled ? 'Turn off' : 'Turn on',
                            icon: rule.enabled ? ('stop' as const) : ('play' as const),
                            disabled: !rule.enabled && !rule.canEdit,
                            onSelect: () => toggle.mutate(rule),
                          },
                          { separator: true },
                          {
                            label: 'Delete',
                            icon: 'trash' as const,
                            danger: true,
                            onSelect: () => setDeleting(rule),
                          },
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
                    <Icon name="checklist" size={16} />
                  </span>
                }
                meta={
                  <span className="aw-row__metaline aw-wrap">
                    <span>
                      {describeTrigger(rule.trigger)}
                      {rule.viewName ? ` in “${rule.viewName}”` : ''}, {rule.actions.map(describeAction).join(', then ')}
                    </span>
                    <span>By {rule.ownerName ?? 'someone who left'}</span>
                    <span title={rule.lastFiredAt ? formatFullDate(rule.lastFiredAt) : undefined}>
                      {rule.lastFiredAt ? `Last ran ${formatRelative(rule.lastFiredAt)}` : 'Hasn’t run yet'}
                    </span>
                    {rule.enabled && rule.nextRunAt && <span title={formatFullDate(rule.nextRunAt)}>Next {formatRelative(rule.nextRunAt)}</span>}
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
          icon="checklist"
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
      <ConfirmDialog
        busy={remove.isPending}
        confirmLabel="Delete rule"
        onClose={() => setDeleting(null)}
        onConfirm={() => deleting && remove.mutate(deleting)}
        open={deleting !== null}
        title={deleting ? `Delete ${deleting.name}?` : 'Delete this rule?'}
      >
        It stops for good, and its history goes too.
      </ConfirmDialog>
    </>
  )
}

function HistoryDialog({ onClose, projectId, rule }: { onClose: () => void; projectId: string; rule: AutomationRule }) {
  const runs = useQuery({
    queryKey: automationKeys.runs(projectId, rule.id),
    queryFn: () => listRuns(projectId, rule.id),
  })
  return (
    <Dialog footer={<Button onClick={onClose}>Done</Button>} onClose={onClose} open title={`History of ${rule.name}`} width={560}>
      {runs.isPending ? (
        <ListSkeleton label="Loading history" rows={3} />
      ) : runs.isError ? (
        <ErrorState error={runs.error} onRetry={() => void runs.refetch()} title="History didn’t load" />
      ) : runs.data.runs.length ? (
        <RowList label="Runs">
          {runs.data.runs.map((entry) => (
            <Row
              key={entry.id}
              meta={
                <span className="aw-row__metaline aw-wrap">
                  <span title={formatFullDate(entry.createdAt)}>{formatRelative(entry.createdAt)}</span>
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
