import { useState, type ReactNode } from 'react'

import { Avatar, AvatarStack, Button, DatePicker, Icon, Popover, TextArea, actorKind } from '../ui'
import { priorityLabel } from './constants'
import { FieldPicker, type PickerField } from './FieldPicker'
import { useSetWatching } from './planningHooks'
import { pickToPatch } from './taskEdits'
import { EstimateField } from './TaskFields'
import { TaskGlyph } from './TaskGlyph'
import { TaskSearchPicker } from './TaskSearchPicker'
import { isOverdue } from './taskDisplay'
import type { WorkItem, WorkItemWorkflow } from './types'
import type { TaskSave } from './useTaskSave'
import { PriorityIcon, StatusIcon } from './WorkItemIcons'
import { WorkItemLabels } from './WorkItemLabels'

function Property({ children, label }: { children: ReactNode; label: string }) {
  return (
    <div className="aw-prop">
      <span className="aw-prop__label">{label}</span>
      <div className="aw-prop__value">{children}</div>
    </div>
  )
}

function PropertyButton({
  children,
  disabled,
  label,
  onOpen,
}: {
  children: ReactNode
  disabled: boolean
  label: string
  onOpen: (anchor: HTMLElement) => void
}) {
  if (disabled) return <span className="aw-prop__static">{children}</span>
  return (
    <button aria-haspopup="dialog" aria-label={label} className="aw-combobox-trigger aw-prop__button" onClick={(event) => onOpen(event.currentTarget)} type="button">
      {children}
    </button>
  )
}

/** The task's fields, each saved the moment it changes. */
export function TaskProperties({
  disabled,
  item,
  onManageLabels,
  onOpenTask,
  projectArchived,
  save,
  workflow,
}: {
  disabled: boolean
  item: WorkItem
  onManageLabels: () => void
  onOpenTask: (task: { id: string; projectId: string; href: string }) => void
  projectArchived: boolean
  save: TaskSave
  workflow: WorkItemWorkflow
}) {
  const [picker, setPicker] = useState<{ field: PickerField; anchor: HTMLElement } | null>(null)
  const [parentAnchor, setParentAnchor] = useState<HTMLElement | null>(null)
  const open = (field: PickerField) => (anchor: HTMLElement) => setPicker({ field, anchor })
  const status = workflow.statuses.find((entry) => entry.id === item.status.id) ?? item.status
  const overdue = isOverdue(item)

  return (
    <div className="aw-props aw-taskprops">
      <Property label="Status">
        <PropertyButton disabled={disabled} label={`Status: ${status.name}. Change`} onOpen={open('status')}>
          <StatusIcon category={status.semanticCategory} />
          {status.name}
        </PropertyButton>
      </Property>
      <Property label="Priority">
        <PropertyButton disabled={disabled} label={`Priority: ${priorityLabel(item.priority)}. Change`} onOpen={open('priority')}>
          <PriorityIcon decorative priority={item.priority} />
          {priorityLabel(item.priority)}
        </PropertyButton>
      </Property>
      <Property label="Assignee">
        <PropertyButton disabled={disabled} label={`Assignee: ${item.assignee?.displayName ?? 'Unassigned'}. Change`} onOpen={open('assignee')}>
          {item.assignee ? (
            <>
              <Avatar kind={actorKind(item.assignee.kind)} name={item.assignee.displayName} seed={item.assignee.actorId} size={20} />
              {item.assignee.displayName}
              {item.assignee.active ? '' : ' (inactive)'}
            </>
          ) : (
            <>
              <Icon name="unassigned" size={16} />
              Unassigned
            </>
          )}
        </PropertyButton>
      </Property>
      <Property label="Type">
        <PropertyButton disabled={disabled} label={`Type: ${item.itemType.name}. Change`} onOpen={open('type')}>
          {item.itemType.name}
        </PropertyButton>
      </Property>
      <Property label="Labels">
        <PropertyButton disabled={disabled} label={`Labels: ${item.labels.map((label) => label.name).join(', ') || 'none'}. Change`} onOpen={open('labels')}>
          {item.labels.length ? <WorkItemLabels labels={item.labels} /> : <span className="aw-prop__empty">Add labels</span>}
        </PropertyButton>
      </Property>
      <Property label="Milestone">
        <PropertyButton disabled={disabled} label={`Milestone: ${item.milestone?.name ?? 'none'}. Change`} onOpen={open('milestone')}>
          <TaskGlyph name="milestone" />
          {item.milestone ? item.milestone.name : <span className="aw-prop__empty">No milestone</span>}
        </PropertyButton>
      </Property>
      <Property label="Parent">
        <span className="aw-prop__pair">
          <PropertyButton disabled={disabled} label={`Parent: ${item.parent ? item.parent.key : 'none'}. Change`} onOpen={setParentAnchor}>
            {item.parent ? <span className="aw-prop__mono">{item.parent.key}</span> : <span className="aw-prop__empty">No parent</span>}
          </PropertyButton>
          {item.parent ? (
            <a
              className="aw-prop__link"
              href={item.parent.href}
              onClick={(event) => {
                if (!item.parent || event.metaKey || event.ctrlKey) return
                event.preventDefault()
                onOpenTask(item.parent)
              }}
            >
              {item.parent.title}
            </a>
          ) : null}
        </span>
      </Property>
      <Property label="Start date">
        <DatePicker disabled={disabled} label="Start date" onChange={(value) => save.save('Start date', { startDate: value })} value={item.startDate} />
      </Property>
      <Property label="Due date">
        <span className={overdue ? 'aw-prop__pair is-overdue' : 'aw-prop__pair'}>
          <DatePicker disabled={disabled} label="Due date" onChange={(value) => save.save('Due date', { dueDate: value })} value={item.dueDate} />
          {overdue ? <span className="aw-due is-overdue">Overdue</span> : null}
        </span>
      </Property>
      <Property label="Estimate">
        <EstimateField disabled={disabled} onSave={(value) => save.save('Estimate', { estimate: value })} unit={item.estimateUnit} value={item.estimate} />
      </Property>
      <Property label="Blocked">
        <BlockedField disabled={disabled} item={item} save={save} />
      </Property>
      <Property label="Watchers">
        <WatchField item={item} readOnly={projectArchived} />
      </Property>

      {picker ? (
        <FieldPicker
          anchor={picker.anchor}
          current={{
            statusId: item.status.id,
            priority: item.priority,
            assigneeId: item.assignee?.actorId ?? null,
            assignee: item.assignee
              ? { id: item.assignee.actorId, displayName: item.assignee.displayName, handle: item.assignee.handle, kind: item.assignee.kind }
              : null,
            labelIds: item.labels.map((label) => label.id),
            milestoneId: item.milestone?.id ?? null,
            typeId: item.itemType.id,
          }}
          field={picker.field}
          onClose={() => setPicker(null)}
          onManageLabels={onManageLabels}
          onPick={(result) => {
            const names: Record<PickerField, string> = { status: 'Status', priority: 'Priority', assignee: 'Assignee', labels: 'Labels', milestone: 'Milestone', type: 'Type' }
            save.save(names[result.field], pickToPatch(result))
          }}
          projectId={item.projectId}
          workflow={workflow}
        />
      ) : null}
      <TaskSearchPicker
        allowTypedKey={false}
        anchor={parentAnchor}
        exclude={[item.id, ...item.subtasks.map((task) => task.id)]}
        label="Set parent"
        noneLabel="No parent"
        onClose={() => setParentAnchor(null)}
        onPick={(target) => {
          setParentAnchor(null)
          save.save('Parent', { parentWorkItemId: target.id })
        }}
        projectId={item.projectId}
      />
    </div>
  )
}

/** The manual "blocked" flag, with the reason people see. */
function BlockedField({ disabled, item, save }: { disabled: boolean; item: WorkItem; save: TaskSave }) {
  const [anchor, setAnchor] = useState<HTMLElement | null>(null)
  const [reason, setReason] = useState('')
  const openBlockers = item.blockedBy.filter((task) => !task.finished).length
  return (
    <span className="aw-blocked">
      {item.blocked ? (
        <>
          <span className="aw-blocked__reason">
            <TaskGlyph name="blocked" />
            {item.blocked.reason || 'Blocked'}
          </span>
          {!disabled ? (
            <Button onClick={() => save.save('Blocked', { blockedReason: null })} size="sm" variant="ghost">
              Unblock
            </Button>
          ) : null}
        </>
      ) : (
        <>
          <span className="aw-prop__empty">{openBlockers ? `Waiting on ${openBlockers} work item${openBlockers === 1 ? '' : 's'}` : item.ready ? 'Ready' : 'No'}</span>
          {!disabled ? (
            <Button
              onClick={(event) => {
                setReason('')
                setAnchor(event.currentTarget)
              }}
              size="sm"
              variant="ghost"
            >
              Mark blocked
            </Button>
          ) : null}
        </>
      )}
      <Popover anchor={anchor} label="Mark as blocked" onClose={() => setAnchor(null)} open={anchor !== null} width={300}>
        <form
          className="aw-blocked__form"
          onSubmit={(event) => {
            event.preventDefault()
            if (!reason.trim()) return
            save.save('Blocked', { blockedReason: reason.trim() })
            setAnchor(null)
          }}
        >
          <TextArea
            label="What’s in the way?"
            maxLength={500}
            onChange={(event) => setReason(event.target.value)}
            placeholder="Waiting on the API keys from the vendor"
            rows={3}
            value={reason}
          />
          <Button disabled={!reason.trim()} size="sm" type="submit" variant="primary">
            Mark blocked
          </Button>
        </form>
      </Popover>
    </span>
  )
}

/** Anyone who can see a task can watch it, but not while its project is archived. */
function WatchField({ item, readOnly }: { item: WorkItem; readOnly: boolean }) {
  const watch = useSetWatching()
  return (
    <span className="aw-watch">
      {item.watchers.length ? (
        <span className="aw-watch__people" title={item.watchers.map((watcher) => watcher.displayName).join(', ')}>
          <AvatarStack people={item.watchers.map((watcher) => ({ id: watcher.id, name: watcher.displayName, kind: actorKind(watcher.kind) }))} size={20} />
          <span className="visually-hidden">Watched by {item.watchers.map((watcher) => watcher.displayName).join(', ')}</span>
          {item.watchers.length > 3 ? <span className="aw-watch__more">+{item.watchers.length - 3}</span> : null}
        </span>
      ) : (
        <span className="aw-prop__empty">Nobody yet</span>
      )}
      {readOnly ? null : (
        <Button
          aria-pressed={item.watching}
          busy={watch.isPending}
          onClick={() => watch.mutate({ projectId: item.projectId, workItemId: item.id, watching: !item.watching, idempotencyKey: crypto.randomUUID() })}
          size="sm"
          variant="ghost"
        >
          <TaskGlyph name={item.watching ? 'unwatch' : 'watch'} />
          {item.watching ? 'Unwatch' : 'Watch'}
        </Button>
      )}
      {watch.isError ? <span role="alert" className="aw-prop__error">That didn’t save. Try again.</span> : null}
    </span>
  )
}
