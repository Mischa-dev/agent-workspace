import { useRef, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from 'react-router-dom'

import { describeError, toastError } from '../../app/mutations'
import { queryKeys } from '../../api/queryKeys'
import {
  Button,
  Checkbox,
  Dialog,
  EmptyState,
  ErrorState,
  Icon,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  Section,
  SelectField,
  TextArea,
  TextField,
  formatDateTime,
  showToast,
} from '../../ui'
import {
  applyTemplate,
  createRecurring,
  createTemplate,
  deleteRecurring,
  saveAsProjectTemplate,
  updateRecurring,
  updateTemplate,
  viewKeys,
  type Recurring,
  type ScheduleDraft,
  type TaskTemplate,
  type TemplateFields,
  listTemplates,
} from '../../views/api'
import { workItemPriorities } from '../../work-items/constants'
import { workItemHref } from '../../work-items/links'
import type { SettingsPageProps } from '../types'

const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']

/** "Every week on Mon, Thu at 09:00 (Europe/London)". */
function describeSchedule(recurring: Pick<Recurring, 'frequency' | 'every' | 'weekdays' | 'monthDay' | 'timeOfDay' | 'timezone'>) {
  const unit = { daily: 'day', weekly: 'week', monthly: 'month' }[recurring.frequency]
  const every = recurring.every === 1 ? `Every ${unit}` : `Every ${recurring.every} ${unit}s`
  const on =
    recurring.frequency === 'weekly' && recurring.weekdays.length
      ? ` on ${recurring.weekdays.map((day) => WEEKDAYS[day - 1]).join(', ')}`
      : recurring.frequency === 'monthly' && recurring.monthDay
        ? ` on day ${recurring.monthDay}`
        : ''
  return `${every}${on} at ${recurring.timeOfDay} (${recurring.timezone})`
}

/** Project › Templates: task templates, recurring tasks, and saving the Project's setup as a template. */
export default function TemplatesPage({ project }: SettingsPageProps) {
  const projectId = project?.id ?? ''
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const templates = useQuery({ queryKey: viewKeys.templates(projectId), queryFn: () => listTemplates(projectId), enabled: Boolean(projectId) })
  const [editing, setEditing] = useState<TaskTemplate | 'new' | null>(null)
  const [scheduling, setScheduling] = useState(false)
  const [savingProject, setSavingProject] = useState(false)
  const refresh = () => queryClient.invalidateQueries({ queryKey: viewKeys.templates(projectId) })

  const use = useMutation({
    mutationFn: (template: TaskTemplate) => applyTemplate(projectId, template.id, {}, crypto.randomUUID()),
    onSuccess: (applied) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) })
      showToast({ tone: 'success', title: `Created ${applied.key}`, action: { label: 'Open', onSelect: () => navigate(workItemHref(projectId, applied.workItemId)) } })
    },
    onError: (error) => toastError('The task wasn’t created', error),
  })
  const archive = useMutation({
    mutationFn: (template: TaskTemplate) => updateTemplate(projectId, template, { archived: !template.archived }, crypto.randomUUID()),
    onSuccess: (template) => showToast({ title: template.archived ? `“${template.name}” archived` : `“${template.name}” restored` }),
    onError: (error) => toastError('That didn’t save', error),
    onSettled: refresh,
  })
  const toggleRecurring = useMutation({
    mutationFn: (recurring: Recurring) => updateRecurring(projectId, recurring, { enabled: !recurring.enabled }, crypto.randomUUID()),
    onSuccess: (recurring) => showToast({ title: recurring.enabled ? `“${recurring.templateName}” resumed` : `“${recurring.templateName}” paused` }),
    onError: (error) => toastError('That didn’t save', error),
    onSettled: refresh,
  })
  const removeRecurring = useMutation({
    mutationFn: (recurring: Recurring) => deleteRecurring(projectId, recurring, crypto.randomUUID()),
    onSuccess: () => showToast({ title: 'Recurring task removed. Tasks it already made stay.' }),
    onError: (error) => toastError('That didn’t delete', error),
    onSettled: refresh,
  })
  if (!project) return null
  const canEdit = Boolean(templates.data?.canEdit)
  const active = (templates.data?.templates ?? []).filter((template) => !template.archived)
  const archived = (templates.data?.templates ?? []).filter((template) => template.archived)

  return (
    <>
      <PageHeader
        actions={
          canEdit ? (
            <Button icon="plus" onClick={() => setEditing('new')} variant="primary">
              New template
            </Button>
          ) : null
        }
        icon="list"
        title="Templates"
      >
        Tasks you make often, ready with their criteria and subtasks, and tasks that make themselves on a schedule.
      </PageHeader>
      {templates.isPending ? <ListSkeleton label="Loading templates" rows={3} /> : null}
      {templates.isError ? (
        <ErrorState action={<Button onClick={() => void templates.refetch()}>Try again</Button>} title="Templates didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : null}
      {templates.data ? (
        <>
          <Section id="task-templates-heading" title="Task templates">
            {active.length ? (
              <RowList label="Task templates">
                {[...active, ...archived].map((template) => (
                  <Row
                    actions={
                      canEdit ? (
                        <>
                          {template.archived ? null : (
                            <Button busy={use.isPending && use.variables?.id === template.id} onClick={() => use.mutate(template)} size="sm">
                              Create task
                            </Button>
                          )}
                          {template.archived ? null : (
                            <Button onClick={() => setEditing(template)} size="sm" variant="ghost">
                              Edit
                            </Button>
                          )}
                          <Button onClick={() => archive.mutate(template)} size="sm" variant="ghost">
                            {template.archived ? 'Restore' : 'Archive'}
                          </Button>
                        </>
                      ) : null
                    }
                    key={template.id}
                    leading={
                      <span className="aw-row__tile">
                        <Icon name={template.archived ? 'archive' : 'list'} size={16} />
                      </span>
                    }
                    meta={
                      <span className="aw-row__metaline aw-wrap">
                        <span>{template.title}</span>
                        {template.acceptanceCriteria.length ? <span>{template.acceptanceCriteria.length} criteria</span> : null}
                        {template.subtasks.length ? <span>{template.subtasks.length} subtasks</span> : null}
                        {template.labels.length ? <span>{template.labels.join(', ')}</span> : null}
                        {template.archived ? <span>Archived</span> : null}
                      </span>
                    }
                    title={template.name}
                  />
                ))}
              </RowList>
            ) : (
              <EmptyState
                action={canEdit ? <Button onClick={() => setEditing('new')}>New template</Button> : undefined}
                icon="list"
                title="No task templates yet"
              >
                A template fills in a task’s title, description, labels, acceptance criteria and subtasks in one go. Agents
                can use them too.
              </EmptyState>
            )}
          </Section>
          <Section
            actions={
              canEdit && active.length ? (
                <Button icon="plus" onClick={() => setScheduling(true)} size="sm">
                  New recurring task
                </Button>
              ) : null
            }
            description="Made from a template on schedule, even if the server was down when it was due."
            id="recurring-heading"
            title="Recurring tasks"
          >
            {templates.data.recurring.length ? (
              <RowList label="Recurring tasks">
                {templates.data.recurring.map((recurring) => (
                  <Row
                    actions={
                      canEdit ? (
                        <>
                          <Button onClick={() => toggleRecurring.mutate(recurring)} size="sm" variant="ghost">
                            {recurring.enabled ? 'Pause' : 'Resume'}
                          </Button>
                          <Button icon="trash" onClick={() => removeRecurring.mutate(recurring)} size="sm" variant="ghost">
                            Remove
                          </Button>
                        </>
                      ) : null
                    }
                    key={recurring.id}
                    leading={
                      <span className="aw-row__tile">
                        <Icon name="clock" size={16} />
                      </span>
                    }
                    meta={
                      <span className="aw-row__metaline aw-wrap">
                        <span>{describeSchedule(recurring)}</span>
                        {recurring.enabled && recurring.nextRunAt ? <span>Next {formatDateTime(recurring.nextRunAt)}</span> : null}
                        {!recurring.enabled ? <span>Paused{recurring.pausedReason ? `: ${recurring.pausedReason}` : ''}</span> : null}
                        {recurring.lastWorkItemId && recurring.lastWorkItemKey ? (
                          <Link to={workItemHref(projectId, recurring.lastWorkItemId)}>Last made {recurring.lastWorkItemKey}</Link>
                        ) : null}
                        {recurring.assigneeName ? <span>For {recurring.assigneeName}</span> : null}
                        {recurring.actsAs ? <span>Set up by {recurring.actsAs}</span> : null}
                      </span>
                    }
                    title={recurring.templateName}
                  />
                ))}
              </RowList>
            ) : (
              <p className="aw-pagesection__desc">{active.length ? 'None yet.' : 'Make a task template first, then schedule it here.'}</p>
            )}
          </Section>
        </>
      ) : null}
      {project.capabilities.manage && !project.archivedAt ? (
        <Section
          actions={
            <Button onClick={() => setSavingProject(true)} size="sm">
              Save as Project template
            </Button>
          }
          description="Keeps this Project’s workflow, labels, channels, defaults and task templates so a new Project can start the same way."
          id="project-template-heading"
          title="Project template"
        >
          <p className="aw-pagesection__desc">Choose the template when you create a Project. Workspace settings list them all.</p>
        </Section>
      ) : null}
      {editing ? (
        <TemplateDialog
          onClose={() => setEditing(null)}
          onSaved={(template) => {
            setEditing(null)
            void refresh()
            showToast({ tone: 'success', title: `“${template.name}” saved` })
          }}
          projectId={projectId}
          template={editing === 'new' ? null : editing}
        />
      ) : null}
      {scheduling ? (
        <RecurringDialog
          onClose={() => setScheduling(false)}
          onSaved={() => {
            setScheduling(false)
            void refresh()
            showToast({ tone: 'success', title: 'Recurring task scheduled' })
          }}
          projectId={projectId}
          templates={active}
        />
      ) : null}
      {savingProject ? (
        <SaveProjectTemplateDialog
          defaultName={project.name}
          onClose={() => setSavingProject(false)}
          projectId={projectId}
        />
      ) : null}
    </>
  )
}

const lines = (value: string) =>
  value
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)

function TemplateDialog({
  onClose,
  onSaved,
  projectId,
  template,
}: {
  onClose: () => void
  onSaved: (template: TaskTemplate) => void
  projectId: string
  template: TaskTemplate | null
}) {
  const idempotencyKey = useRef(crypto.randomUUID())
  const [name, setName] = useState(template?.name ?? '')
  const [title, setTitle] = useState(template?.title ?? '')
  const [description, setDescription] = useState(template?.description ?? '')
  const [priority, setPriority] = useState(template?.priority ?? 'no_priority')
  const [labels, setLabels] = useState(template?.labels.join(', ') ?? '')
  const [criteria, setCriteria] = useState(template?.acceptanceCriteria.join('\n') ?? '')
  const [subtasks, setSubtasks] = useState(template?.subtasks.map((subtask) => subtask.title).join('\n') ?? '')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (busy || !name.trim() || !title.trim()) return
    setBusy(true)
    setError(null)
    const kept = new Map((template?.subtasks ?? []).map((subtask) => [subtask.title, subtask.description]))
    const fields: TemplateFields = {
      name: name.trim(),
      title: title.trim(),
      description,
      typeId: template?.typeId ?? null,
      priority,
      labels: labels
        .split(',')
        .map((label) => label.trim())
        .filter(Boolean),
      acceptanceCriteria: lines(criteria),
      subtasks: lines(subtasks).map((line) => ({ title: line, description: kept.get(line) ?? '' })),
      estimate: template?.estimate ?? null,
    }
    try {
      onSaved(template ? await updateTemplate(projectId, template, fields, idempotencyKey.current) : await createTemplate(projectId, fields, idempotencyKey.current))
    } catch (reason) {
      idempotencyKey.current = crypto.randomUUID()
      setError(describeError(reason).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} disabled={!name.trim() || !title.trim()} type="submit" variant="primary">
            Save template
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => void submit(event)}
      open
      title={template ? 'Edit template' : 'New task template'}
      width={560}
    >
      <TextField autoFocus error={error ?? undefined} label="Template name" maxLength={80} onChange={(event) => setName(event.target.value)} placeholder="e.g. Weekly release check" value={name} />
      <TextField label="Task title" maxLength={500} onChange={(event) => setTitle(event.target.value)} value={title} />
      <TextArea label="Description" onChange={(event) => setDescription(event.target.value)} rows={4} value={description} />
      <SelectField label="Priority" onChange={(event) => setPriority(event.target.value)} value={priority}>
        {workItemPriorities.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </SelectField>
      <TextField hint="Separate with commas. Missing labels are made when the template is used." label="Labels" onChange={(event) => setLabels(event.target.value)} value={labels} />
      <TextArea hint="One per line." label="Acceptance criteria" onChange={(event) => setCriteria(event.target.value)} rows={3} value={criteria} />
      <TextArea hint="One per line. Each becomes a task under the new one." label="Subtasks" onChange={(event) => setSubtasks(event.target.value)} rows={3} value={subtasks} />
    </Dialog>
  )
}

function localTimezone() {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
  } catch {
    return 'UTC'
  }
}

function RecurringDialog({
  onClose,
  onSaved,
  projectId,
  templates,
}: {
  onClose: () => void
  onSaved: () => void
  projectId: string
  templates: TaskTemplate[]
}) {
  const idempotencyKey = useRef(crypto.randomUUID())
  const [templateId, setTemplateId] = useState(templates[0]?.id ?? '')
  const [schedule, setSchedule] = useState<ScheduleDraft>({
    frequency: 'weekly',
    every: 1,
    weekdays: [1],
    monthDay: 1,
    timeOfDay: '09:00',
    timezone: localTimezone(),
    dueInDays: null,
  })
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const change = (next: Partial<ScheduleDraft>) => setSchedule((current) => ({ ...current, ...next }))

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (busy || !templateId) return
    setBusy(true)
    setError(null)
    try {
      await createRecurring(
        projectId,
        templateId,
        {
          ...schedule,
          weekdays: schedule.frequency === 'weekly' ? schedule.weekdays : [],
          monthDay: schedule.frequency === 'monthly' ? schedule.monthDay : null,
        },
        idempotencyKey.current,
      )
      onSaved()
    } catch (reason) {
      idempotencyKey.current = crypto.randomUUID()
      setError(describeError(reason).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      description="It runs with your access. If you leave the Project, it pauses."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} disabled={!templateId || (schedule.frequency === 'weekly' && !schedule.weekdays.length)} type="submit" variant="primary">
            Schedule
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => void submit(event)}
      open
      title="New recurring task"
    >
      <SelectField error={error ?? undefined} label="Template" onChange={(event) => setTemplateId(event.target.value)} value={templateId}>
        {templates.map((template) => (
          <option key={template.id} value={template.id}>
            {template.name}
          </option>
        ))}
      </SelectField>
      <div className="aw-form__row">
        <SelectField label="Repeats" onChange={(event) => change({ frequency: event.target.value as ScheduleDraft['frequency'] })} value={schedule.frequency}>
          <option value="daily">Daily</option>
          <option value="weekly">Weekly</option>
          <option value="monthly">Monthly</option>
        </SelectField>
        <TextField label="Every" max={52} min={1} onChange={(event) => change({ every: Math.max(1, Number(event.target.value) || 1) })} type="number" value={schedule.every} />
      </div>
      {schedule.frequency === 'weekly' ? (
        <fieldset style={{ border: 0, margin: 0, padding: 0 }}>
          <legend className="aw-field__label">On</legend>
          <div className="aw-form__row">
            {WEEKDAYS.map((label, index) => (
              <Checkbox
                checked={schedule.weekdays.includes(index + 1)}
                key={label}
                label={label}
                onChange={(checked) =>
                  change({ weekdays: checked ? [...schedule.weekdays, index + 1].sort() : schedule.weekdays.filter((day) => day !== index + 1) })
                }
              />
            ))}
          </div>
        </fieldset>
      ) : null}
      {schedule.frequency === 'monthly' ? (
        <TextField
          hint="Shorter months use their last day."
          label="Day of the month"
          max={31}
          min={1}
          onChange={(event) => change({ monthDay: Math.min(31, Math.max(1, Number(event.target.value) || 1)) })}
          type="number"
          value={schedule.monthDay ?? 1}
        />
      ) : null}
      <div className="aw-form__row">
        <TextField label="At" onChange={(event) => change({ timeOfDay: event.target.value })} type="time" value={schedule.timeOfDay} />
        <TextField label="Time zone" onChange={(event) => change({ timezone: event.target.value })} value={schedule.timezone} />
      </div>
      <TextField
        hint="Leave empty for no due date."
        label="Due after (days)"
        min={0}
        onChange={(event) => change({ dueInDays: event.target.value === '' ? null : Math.max(0, Number(event.target.value) || 0) })}
        type="number"
        value={schedule.dueInDays ?? ''}
      />
    </Dialog>
  )
}

function SaveProjectTemplateDialog({ defaultName, onClose, projectId }: { defaultName: string; onClose: () => void; projectId: string }) {
  const queryClient = useQueryClient()
  const idempotencyKey = useRef(crypto.randomUUID())
  const [name, setName] = useState(`${defaultName} setup`)
  const [description, setDescription] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (busy || !name.trim()) return
    setBusy(true)
    setError(null)
    try {
      const template = await saveAsProjectTemplate(projectId, { name: name.trim(), description }, idempotencyKey.current)
      void queryClient.invalidateQueries({ queryKey: ['organization', template.organizationId, 'project-templates'] })
      showToast({ tone: 'success', title: `“${template.name}” saved`, description: 'Pick it when you create a Project.' })
      onClose()
    } catch (reason) {
      idempotencyKey.current = crypto.randomUUID()
      setError(describeError(reason).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      description="Saves the workflow, labels, channel names, Project defaults and task templates. Not tasks, members, repositories or keys."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} disabled={!name.trim()} type="submit" variant="primary">
            Save template
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => void submit(event)}
      open
      title="Save as Project template"
    >
      <TextField autoFocus error={error ?? undefined} label="Name" maxLength={80} onChange={(event) => setName(event.target.value)} value={name} />
      <TextArea label="Description" maxLength={2000} onChange={(event) => setDescription(event.target.value)} rows={3} value={description} />
    </Dialog>
  )
}
