import { useRef, useState, type ReactNode } from 'react'

import { useProjectExperience } from '../experience/layout'
import { Avatar, Icon, actorKind } from '../ui'
import { DatePicker } from '../ui/pickers'
import { priorityLabel } from './constants'
import { FieldPicker, type PickerField } from './FieldPicker'
import { pickToPatch } from './taskEdits'
import { useFields } from '../fields/api'
import { isEmpty } from '../fields/fieldModel'
import { TaskCustomFields } from './TaskCustomFields'
import { EstimateField } from './TaskFields'
import { TaskGlyph } from './TaskGlyph'
import { isOverdue } from './taskDisplay'
import type { WorkItem, WorkItemWorkflow } from './types'
import type { TaskSave } from './useTaskSave'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'
import { WorkItemLabels } from './WorkItemLabels'

/**
 * The task's fields, each saved the moment it changes: labelled rows in the
 * full page's side rail, or a wrap of compact pills under the title in the
 * side panel and on phones.
 */
export function TaskProperties({
  disabled,
  item,
  onManageLabels,
  save,
  variant,
  workflow,
}: {
  disabled: boolean
  item: WorkItem
  onManageLabels: () => void
  save: TaskSave
  variant: 'rail' | 'pills'
  workflow: WorkItemWorkflow
}) {
  const [picker, setPicker] = useState<{ field: PickerField; anchor: HTMLElement } | null>(null)
  const [more, setMore] = useState(false)
  const experience = useProjectExperience(item.projectId)
  const status = workflow.statuses.find((entry) => entry.id === item.status.id) ?? item.status
  const overdue = isOverdue(item)
  const rail = variant === 'rail'
  const control = rail ? 'aw-prop__button' : 'aw-pill'

  // In pills, the fields people set less often wait behind More while they're empty.
  const tucked = (empty: boolean) => !rail && !more && !disabled && empty
  const showMilestone = experience.shows('task.milestone')
  const showDates = experience.shows('task.dates')
  const showEstimate = experience.shows('task.estimate')
  const showCustom = experience.shows('task.custom_fields')
  const customFields = useFields({ projectId: item.projectId }).data?.fields ?? []
  const hidden = [
    showMilestone && tucked(!item.milestone) ? 'Milestone' : null,
    showDates && tucked(!item.startDate) ? 'Start date' : null,
    showEstimate && tucked(item.estimate === null) ? 'Estimate' : null,
    ...(showCustom ? customFields : [])
      .filter((custom) => tucked(isEmpty(item.fieldValues[custom.id])))
      .map((custom) => custom.name),
  ].filter((name) => name !== null)
  const group = useRef<HTMLDivElement>(null)
  const showMore = () => {
    setMore(true)
    // The button goes away, so focus moves to the first field it showed.
    window.requestAnimationFrame(() => group.current?.querySelector<HTMLElement>(`[data-field="${hidden[0]}"] button`)?.focus())
  }
  // A workflow with one type doesn't need to say it on every task.
  const showType =
    experience.shows('task.type') && (workflow.types.length > 1 || !workflow.types.some((type) => type.id === item.itemType.id))

  // One field: a label beside it in the rail; in pills the value carries its own name.
  const field = (label: string, value: ReactNode) =>
    rail ? (
      <div className="aw-prop" key={label}>
        <span className="aw-prop__label">{label}</span>
        <div className="aw-prop__value">{value}</div>
      </div>
    ) : (
      <span className="aw-pills__item" data-field={label} key={label}>
        {value}
      </span>
    )

  const pickerButton = (name: PickerField, label: string, content: ReactNode, empty = false) =>
    disabled ? (
      <span className={`${control} is-static${empty ? ' is-empty' : ''}`}>{content}</span>
    ) : (
      <button
        aria-haspopup="dialog"
        aria-label={label}
        className={`${control}${empty ? ' is-empty' : ''}`}
        onClick={(event) => setPicker({ field: name, anchor: event.currentTarget })}
        type="button"
      >
        {content}
      </button>
    )

  const assignee = item.assignee
  const fields = [
    field(
      'Status',
      pickerButton(
        'status',
        `Status: ${status.name}. Change`,
        <>
          <StatusIcon category={status.semanticCategory} />
          {status.name}
        </>,
      ),
    ),
    experience.shows('task.priority')
      ? field(
          'Priority',
          pickerButton(
            'priority',
            `Priority: ${priorityLabel(item.priority)}. Change`,
            <>
              <PriorityIcon decorative priority={item.priority} />
              {item.priority === 'no_priority' && !rail ? 'Priority' : priorityLabel(item.priority)}
            </>,
            item.priority === 'no_priority',
          ),
        )
      : null,
    field(
      'Assignee',
      pickerButton(
        'assignee',
        `Assignee: ${assignee?.displayName ?? 'Unassigned'}. Change`,
        assignee ? (
          <>
            <Avatar kind={actorKind(assignee.kind)} name={assignee.displayName} seed={assignee.actorId} size={18} />
            <span className="aw-prop__text">
              {assignee.displayName}
              {assignee.active ? '' : ' (inactive)'}
            </span>
          </>
        ) : (
          <>
            <Icon name="unassigned" size={16} />
            {rail ? 'Unassigned' : 'Assignee'}
          </>
        ),
        !assignee,
      ),
    ),
    experience.shows('task.labels')
      ? field(
          'Labels',
          pickerButton(
            'labels',
            `Labels: ${item.labels.map((label) => label.name).join(', ') || 'none'}. Change`,
            item.labels.length ? (
              <WorkItemLabels labels={item.labels} />
            ) : (
              <>
                <TaskGlyph name="tag" />
                {rail ? 'Add labels' : 'Labels'}
              </>
            ),
            !item.labels.length,
          ),
        )
      : null,
    !showMilestone || hidden.includes('Milestone')
      ? null
      : field(
          'Milestone',
          pickerButton(
            'milestone',
            `Milestone: ${item.milestone?.name ?? 'none'}. Change`,
            <>
              <TaskGlyph name="milestone" />
              <span className="aw-prop__text">{item.milestone ? item.milestone.name : rail ? 'No milestone' : 'Milestone'}</span>
            </>,
            !item.milestone,
          ),
        ),
    !showDates || hidden.includes('Start date')
      ? null
      : field(
          'Start date',
          <DatePicker
            className={`${control} aw-prop__date`}
            disabled={disabled}
            label="Start date"
            onChange={(value) => save.save('startDate', { startDate: value })}
            placeholder={rail ? 'No date' : 'Start date'}
            prefix={rail ? undefined : 'Starts'}
            value={item.startDate}
          />,
        ),
    showDates
      ? field(
          'Due date',
          <>
            <DatePicker
              className={`${control} aw-prop__date${overdue ? ' is-overdue' : ''}`}
              disabled={disabled}
              label="Due date"
              onChange={(value) => save.save('dueDate', { dueDate: value })}
              placeholder={rail ? 'No date' : 'Due date'}
              prefix={rail ? undefined : overdue ? 'Overdue' : 'Due'}
              value={item.dueDate}
            />
            {rail && overdue ? <span className="aw-due is-overdue">Overdue</span> : null}
          </>,
        )
      : null,
    !showEstimate || hidden.includes('Estimate')
      ? null
      : field(
          'Estimate',
          <EstimateField
            className={control}
            disabled={disabled}
            onSave={(value) => save.save('estimate', { estimate: value })}
            placeholder={rail ? 'None' : 'Estimate'}
            unit={item.estimateUnit}
            value={item.estimate}
          />,
        ),
    showType
      ? field('Type', pickerButton('type', `Type: ${item.itemType.name}. Change`, <span className="aw-prop__text">{item.itemType.name}</span>))
      : null,
    showCustom ? (
      <TaskCustomFields
        control={control}
        disabled={disabled}
        item={item}
        key="custom-fields"
        render={field}
        save={save}
        tucked={!rail && !more && !disabled}
      />
    ) : null,
    hidden.length ? (
      <span className="aw-pills__item" key="more">
        <button aria-label={`More fields: ${hidden.join(', ').toLowerCase()}`} className="aw-pill is-empty" onClick={showMore} type="button">
          <Icon name="plus" size={14} />
          More
        </button>
      </span>
    ) : null,
  ]

  return (
    <div aria-label={rail ? undefined : 'Task details'} className={`aw-taskprops aw-taskprops--${variant}`} ref={group} role={rail ? undefined : 'group'}>
      {fields}
      {picker ? (
        <FieldPicker
          anchor={picker.anchor}
          current={{
            statusId: item.status.id,
            priority: item.priority,
            assigneeId: assignee?.actorId ?? null,
            assignee: assignee ? { id: assignee.actorId, displayName: assignee.displayName, handle: assignee.handle, kind: assignee.kind } : null,
            labelIds: item.labels.map((label) => label.id),
            milestoneId: item.milestone?.id ?? null,
            typeId: item.itemType.id,
          }}
          field={picker.field}
          onClose={() => setPicker(null)}
          onManageLabels={onManageLabels}
          onPick={(result) => save.save(result.field, pickToPatch(result))}
          projectId={item.projectId}
          workflow={workflow}
        />
      ) : null}
    </div>
  )
}
