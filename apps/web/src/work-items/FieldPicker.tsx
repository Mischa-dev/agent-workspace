import { useEffect, useMemo, useRef, useState } from 'react'

import { Avatar, Icon, actorKind } from '../ui'
import { ComboboxList, Popover, type ComboboxOption } from '../ui/pickers'
import { workItemPriorities } from './constants'
import { useAssignableActors } from './hooks'
import { useCreateLabel, useLabels, useMilestones } from './planningHooks'
import { TaskGlyph } from './TaskGlyph'
import type { AssignableActor, MilestoneRef, WorkItemPriority, WorkItemStatus, WorkItemWorkflow } from './types'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'
import { NONE, PICKER_LABELS, allowedStatuses, useDebouncedValue } from './pickerModel'

export type PickerField = 'status' | 'priority' | 'assignee' | 'labels' | 'milestone' | 'type'

export type PickerCurrent = {
  statusId?: string
  priority?: WorkItemPriority
  assigneeId?: string | null
  /** Shown first in the assignee list even when the search doesn't return them. */
  assignee?: AssignableActor | null
  labelIds?: string[]
  milestoneId?: string | null
  typeId?: string
}

export type PickResult =
  | { field: 'status'; status: WorkItemStatus }
  | { field: 'priority'; priority: WorkItemPriority }
  | { field: 'assignee'; actor: AssignableActor | null }
  | { field: 'milestone'; milestone: MilestoneRef | null }
  | { field: 'type'; type: { id: string; name: string } }
  | { field: 'labels'; add: string[]; remove: string[] }

/**
 * The picker for one field of one or more tasks, in a popover. Single-value
 * fields pick and close; labels toggle freely and are sent once, on close.
 */
export function FieldPicker({
  anchor,
  current,
  field,
  onClose,
  onManageLabels,
  onPick,
  projectId,
  workflow,
}: {
  anchor: HTMLElement | null
  current: PickerCurrent
  field: PickerField
  onClose: () => void
  onManageLabels?: () => void
  onPick: (result: PickResult) => void
  projectId: string
  workflow: WorkItemWorkflow
}) {
  const label = PICKER_LABELS[field]
  const [labelIds, setLabelIds] = useState<string[]>(current.labelIds ?? [])
  const labelState = useRef(labelIds)
  useEffect(() => {
    labelState.current = labelIds
  }, [labelIds])

  const close = () => {
    if (field === 'labels') {
      const before = new Set(current.labelIds ?? [])
      const after = new Set(labelState.current)
      const add = [...after].filter((id) => !before.has(id))
      const remove = [...before].filter((id) => !after.has(id))
      if (add.length || remove.length) onPick({ field: 'labels', add, remove })
    }
    onClose()
  }

  return (
    <Popover anchor={anchor} label={label} onClose={close} open={anchor !== null} width={300}>
      {field === 'status' ? <StatusOptions current={current} label={label} onClose={close} onPick={onPick} workflow={workflow} /> : null}
      {field === 'priority' ? (
        <ComboboxList
          label={label}
          onClose={close}
          onSelect={(value) => onPick({ field: 'priority', priority: value as WorkItemPriority })}
          options={workItemPriorities.map((priority) => ({
            value: priority.value,
            label: priority.label,
            leading: <PriorityIcon decorative priority={priority.value} />,
          }))}
          placeholder="Priority"
          selected={current.priority ? [current.priority] : []}
        />
      ) : null}
      {field === 'type' ? (
        <ComboboxList
          label={label}
          onClose={close}
          onSelect={(value) => {
            const type = workflow.types.find((candidate) => candidate.id === value)
            if (type) onPick({ field: 'type', type: { id: type.id, name: type.name } })
          }}
          options={workflow.types.map((type) => ({ value: type.id, label: type.name }))}
          placeholder="Type"
          selected={current.typeId ? [current.typeId] : []}
        />
      ) : null}
      {field === 'assignee' ? <AssigneeOptions current={current} label={label} onClose={close} onPick={onPick} projectId={projectId} /> : null}
      {field === 'milestone' ? <MilestoneOptions current={current} label={label} onClose={close} onPick={onPick} projectId={projectId} /> : null}
      {field === 'labels' ? (
        <LabelOptions
          label={label}
          onManage={
            onManageLabels
              ? () => {
                  close()
                  onManageLabels()
                }
              : undefined
          }
          projectId={projectId}
          selected={labelIds}
          setSelected={setLabelIds}
        />
      ) : null}
    </Popover>
  )
}

function StatusOptions({
  current,
  label,
  onClose,
  onPick,
  workflow,
}: {
  current: PickerCurrent
  label: string
  onClose: () => void
  onPick: (result: PickResult) => void
  workflow: WorkItemWorkflow
}) {
  const statuses = allowedStatuses(workflow, current.statusId)
  return (
    <ComboboxList
      emptyText="No statuses this task can move to"
      label={label}
      onClose={onClose}
      onSelect={(value) => {
        const status = workflow.statuses.find((candidate) => candidate.id === value)
        if (status) onPick({ field: 'status', status: { id: status.id, name: status.name, semanticCategory: status.semanticCategory } })
      }}
      options={statuses.map((status) => ({
        value: status.id,
        label: status.name,
        leading: <StatusIcon category={status.semanticCategory} />,
      }))}
      placeholder="Status"
      selected={current.statusId ? [current.statusId] : []}
    />
  )
}

function AssigneeOptions({
  current,
  label,
  onClose,
  onPick,
  projectId,
}: {
  current: PickerCurrent
  label: string
  onClose: () => void
  onPick: (result: PickResult) => void
  projectId: string
}) {
  const [query, setQuery] = useState('')
  const debounced = useDebouncedValue(query, 200)
  const actors = useAssignableActors(projectId, debounced)
  const list = useMemo(() => {
    const found = actors.data ?? []
    const pinned = current.assignee && !found.some((actor) => actor.id === current.assignee?.id) && !debounced.trim() ? [current.assignee] : []
    return [...pinned, ...found]
  }, [actors.data, current.assignee, debounced])
  const options: ComboboxOption[] = [
    ...(debounced.trim() ? [] : [{ value: NONE, label: 'Unassigned', leading: <Icon name="unassigned" size={16} /> }]),
    ...list.map((actor) => ({
      value: actor.id,
      label: actor.displayName,
      description: actor.handle ? `@${actor.handle}` : undefined,
      leading: <Avatar kind={actorKind(actor.kind)} name={actor.displayName} seed={actor.id} size={18} />,
    })),
  ]
  return (
    <ComboboxList
      emptyText={actors.isError ? 'People didn’t load. Try again.' : 'Nobody matches'}
      label={label}
      loading={actors.isFetching}
      onClose={onClose}
      onQueryChange={setQuery}
      onSelect={(value) => onPick({ field: 'assignee', actor: value === NONE ? null : (list.find((actor) => actor.id === value) ?? null) })}
      options={options}
      placeholder="Search people and agents"
      selected={[current.assigneeId ?? NONE]}
    />
  )
}

function MilestoneOptions({
  current,
  label,
  onClose,
  onPick,
  projectId,
}: {
  current: PickerCurrent
  label: string
  onClose: () => void
  onPick: (result: PickResult) => void
  projectId: string
}) {
  const milestones = useMilestones(projectId)
  const options: ComboboxOption[] = [
    { value: NONE, label: 'No milestone' },
    ...(milestones.data ?? []).map((milestone) => ({
      value: milestone.id,
      label: milestone.name,
      description: milestone.targetDate ?? undefined,
      leading: <TaskGlyph name="milestone" />,
    })),
  ]
  return (
    <ComboboxList
      emptyText={milestones.isError ? 'Milestones didn’t load.' : 'No milestones yet'}
      label={label}
      loading={milestones.isPending}
      onClose={onClose}
      onSelect={(value) => {
        if (value === NONE) return onPick({ field: 'milestone', milestone: null })
        const milestone = milestones.data?.find((candidate) => candidate.id === value)
        if (milestone) onPick({ field: 'milestone', milestone: { id: milestone.id, name: milestone.name, targetDate: milestone.targetDate } })
      }}
      options={options}
      placeholder="Milestone"
      selected={[current.milestoneId ?? NONE]}
    />
  )
}

function LabelOptions({
  label,
  onManage,
  projectId,
  selected,
  setSelected,
}: {
  label: string
  onManage?: () => void
  projectId: string
  selected: string[]
  setSelected: (next: string[]) => void
}) {
  const labels = useLabels(projectId)
  const create = useCreateLabel()
  return (
    <ComboboxList
      emptyText={labels.isError ? 'Labels didn’t load.' : 'No labels yet'}
      footer={
        onManage ? (
          <button className="aw-btn aw-btn--ghost aw-btn--sm" onClick={onManage} type="button">
            <TaskGlyph name="tag" />
            <span className="aw-btn__label">Manage labels</span>
          </button>
        ) : null
      }
      label={label}
      loading={labels.isPending || create.isPending}
      multiple
      onCreate={(name) =>
        create.mutate(
          { projectId, name, idempotencyKey: crypto.randomUUID() },
          { onSuccess: (created) => setSelected([...selected, created.id]) },
        )
      }
      onSelect={(value) => setSelected(selected.includes(value) ? selected.filter((id) => id !== value) : [...selected, value])}
      options={(labels.data ?? []).map((entry) => ({
        value: entry.id,
        label: entry.name,
        leading: <span aria-hidden="true" className="aw-label__dot" style={{ backgroundColor: entry.color }} />,
      }))}
      placeholder="Search or create labels"
      selected={selected}
    />
  )
}
