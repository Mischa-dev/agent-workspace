import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import { callOp, type OpOutput } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
import { Banner, Button, IconButton, Section, SelectField, Switch, TextField, describeError, showToast } from '../ui'
import { priorityLabel, workItemPriorities } from './constants'
import { useAssignableActors } from './hooks'
import { describeFilter, ruleFilter } from './triageModel'
import type { WorkItemPriority, WorkItemWorkflow } from './types'

type Rule = OpOutput<'triage_rule_list'>['rules'][number]

const rulesKey = (projectId: string) => ['project', projectId, 'triage-rules'] as const

/**
 * A space's triage queue (T1.23): turning it on, and the rules that run in
 * order over each task landing in it, the first that matches acting.
 */
export function TriageSettings({ projectId, workflow }: { projectId: string; workflow: WorkItemWorkflow }) {
  const client = useQueryClient()
  const rules = useQuery({ queryKey: rulesKey(projectId), queryFn: () => callOp('triage_rule_list', { spaceId: projectId }) })
  const people = useAssignableActors(projectId, '')
  const [adding, setAdding] = useState(false)
  const refresh = () => {
    void client.invalidateQueries({ queryKey: rulesKey(projectId) })
    void client.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) })
    void client.invalidateQueries({ queryKey: queryKeys.workItemWorkflow(projectId) })
  }
  const enable = useMutation({
    mutationFn: () => callOp('triage_enable', { spaceId: projectId }, { requestId: crypto.randomUUID() }),
    onSuccess: () => {
      showToast({ tone: 'success', title: 'Triage is on' })
      refresh()
    },
  })
  const write = useMutation({
    mutationFn: (input: Omit<Parameters<typeof callOp<'triage_rule_set'>>[1], 'spaceId'>) =>
      callOp('triage_rule_set', { spaceId: projectId, ...input }, { requestId: crypto.randomUUID() }),
    onSuccess: refresh,
    onError: (error) => showToast({ tone: 'danger', title: describeError(error).message }),
  })
  const enabled = Boolean(rules.data?.triageStatusId)
  const canEdit = workflow.canEdit
  const typeName = (id: string) => workflow.types.find((type) => type.id === id)?.name
  const personName = (id: string | null | undefined) => (people.data ?? []).find((person) => person.id === id)?.displayName ?? 'someone'
  const statusName = (reference: string | null | undefined) =>
    workflow.statuses.find((status) => status.id === reference || status.name.toLowerCase() === reference?.toLowerCase())?.name ?? reference
  const actionText = (rule: Rule) =>
    rule.action.kind === 'accept'
      ? `accept${rule.action.status ? ` to ${statusName(rule.action.status)}` : ''}`
      : rule.action.kind === 'assign'
        ? `give to ${personName(rule.action.to)}`
        : 'keep in triage'

  return (
    <Section
      actions={
        canEdit && enabled ? (
          <Button disabled={adding} icon="plus" onClick={() => setAdding(true)} size="sm">
            Add rule
          </Button>
        ) : null
      }
      description="Tasks from forms, bug report links and imports wait here for someone to accept, decline or snooze them. Rules run in order on each one that arrives, and the first that matches acts."
      id="workflow-triage-heading"
      title="Triage"
    >
      {rules.isError ? <Banner tone="danger">{describeError(rules.error).message}</Banner> : null}
      {rules.data && !enabled ? (
        canEdit ? (
          <Button busy={enable.isPending} onClick={() => enable.mutate()}>
            Turn on triage
          </Button>
        ) : (
          <p className="aw-triagerules__note">This space doesn’t use triage.</p>
        )
      ) : null}
      {enable.isError ? <Banner tone="danger">{describeError(enable.error).message}</Banner> : null}
      {enabled && rules.data ? (
        rules.data.rules.length === 0 && !adding ? (
          <p className="aw-triagerules__note">No rules: every task that arrives waits for a person.</p>
        ) : (
          <ol className="aw-wf__entries aw-triagerules">
            {rules.data.rules.map((rule, index, all) => (
              <li className="aw-triagerules__rule" key={rule.id}>
                <span className="aw-triagerules__text">
                  <strong>{rule.name}</strong>
                  <span>
                    {describeFilter(rule.filter, (value) => priorityLabel(value as WorkItemPriority), typeName)}: {actionText(rule)}
                  </span>
                </span>
                <Switch
                  checked={rule.enabled}
                  disabled={!canEdit || write.isPending}
                  label="On"
                  onChange={(on) => write.mutate({ rule: rule.id, enabled: on, expectedVersion: rule.version })}
                />
                <IconButton disabled={!canEdit || index === 0 || write.isPending} icon="arrow-up" label={`Run ${rule.name} earlier`} onClick={() => write.mutate({ rule: rule.id, position: index, expectedVersion: rule.version })} size="sm" />
                <IconButton
                  disabled={!canEdit || index === all.length - 1 || write.isPending}
                  icon="arrow-down"
                  label={`Run ${rule.name} later`}
                  onClick={() => write.mutate({ rule: rule.id, position: index + 2, expectedVersion: rule.version })}
                  size="sm"
                />
                <IconButton disabled={!canEdit || write.isPending} icon="trash" label={`Remove ${rule.name}`} onClick={() => write.mutate({ rule: rule.id, remove: true })} size="sm" />
              </li>
            ))}
          </ol>
        )
      ) : null}
      {adding ? (
        <RuleForm
          busy={write.isPending}
          onCancel={() => setAdding(false)}
          onSave={(input) => write.mutate(input, { onSuccess: () => setAdding(false) })}
          people={people.data ?? []}
          workflow={workflow}
        />
      ) : null}
    </Section>
  )
}

function RuleForm({
  busy,
  onCancel,
  onSave,
  people,
  workflow,
}: {
  busy: boolean
  onCancel: () => void
  onSave: (input: { name: string; filter: unknown; action: Rule['action'] }) => void
  people: Array<{ id: string; displayName: string }>
  workflow: WorkItemWorkflow
}) {
  const [name, setName] = useState('')
  const [priority, setPriority] = useState('')
  const [typeId, setTypeId] = useState('')
  const [kind, setKind] = useState<Rule['action']['kind']>('accept')
  const [status, setStatus] = useState('')
  const [to, setTo] = useState('')
  const ready = name.trim().length > 0 && (kind !== 'assign' || Boolean(to))
  return (
    <form
      className="aw-triagerules__form"
      onSubmit={(event) => {
        event.preventDefault()
        if (!ready) return
        onSave({
          name: name.trim(),
          filter: ruleFilter(priority, typeId),
          action: { kind, status: kind === 'accept' && status ? status : undefined, to: kind === 'assign' ? to : undefined },
        })
      }}
    >
      <TextField autoFocus label="Name" maxLength={80} onChange={(event) => setName(event.target.value)} value={name} />
      <SelectField label="When priority is" onChange={(event) => setPriority(event.target.value)} value={priority}>
        <option value="">Any</option>
        {workItemPriorities
          .filter((entry) => entry.value !== 'no_priority')
          .map((entry) => (
            <option key={entry.value} value={entry.value}>
              {entry.label}
            </option>
          ))}
      </SelectField>
      <SelectField label="And type is" onChange={(event) => setTypeId(event.target.value)} value={typeId}>
        <option value="">Any</option>
        {workflow.types.map((type) => (
          <option key={type.id} value={type.id}>
            {type.name}
          </option>
        ))}
      </SelectField>
      <SelectField label="Then" onChange={(event) => setKind(event.target.value as Rule['action']['kind'])} value={kind}>
        <option value="accept">Accept it</option>
        <option value="assign">Give it to someone</option>
        <option value="keep">Keep it for a person</option>
      </SelectField>
      {kind === 'accept' ? (
        <SelectField label="Into" onChange={(event) => setStatus(event.target.value)} value={status}>
          <option value="">The first not-started status</option>
          {workflow.statuses
            .filter((entry) => entry.semanticCategory !== 'triage')
            .map((entry) => (
              <option key={entry.id} value={entry.id}>
                {entry.name}
              </option>
            ))}
        </SelectField>
      ) : null}
      {kind === 'assign' ? (
        <SelectField label="To" onChange={(event) => setTo(event.target.value)} value={to}>
          <option disabled value="">
            Choose…
          </option>
          {people.map((person) => (
            <option key={person.id} value={person.id}>
              {person.displayName}
            </option>
          ))}
        </SelectField>
      ) : null}
      <div className="aw-triagerules__buttons">
        <Button onClick={onCancel} variant="ghost">
          Cancel
        </Button>
        <Button busy={busy} disabled={!ready} type="submit" variant="primary">
          Add rule
        </Button>
      </div>
    </form>
  )
}
