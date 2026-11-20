import { useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { opPrefix, opQuery, useOp } from '../api/ops'
import { useWorkspaceId } from '../app/workspace'
import { answerBody, normalizeQuestions, type Answers } from '../forms/formModel'
import { Questions } from '../forms/Questions'
import {
  Banner,
  Button,
  Checkbox,
  Dialog,
  EmptyState,
  ErrorState,
  IconButton,
  ListSkeleton,
  Menu,
  Row,
  RowList,
  SegmentedControl,
  StatusChip,
  TextField,
  describeError,
  showToast,
} from '../ui'
import type { CollectionBody } from './api'
import { collectionKeys } from './keys'
import { describeSchedule, scheduleInput, WEEKDAYS, type Frequency, type ScheduleDraft } from './templateModel'

type Step = { kind: 'list' } | { kind: 'edit'; templateId: string | null } | { kind: 'repeat'; templateId: string; name: string }

/**
 * A collection's record templates (T1.30): records to start from, and the
 * schedules that make one from a template every day, week or month, as
 * whoever set them up.
 */
export function TemplatesDialog({
  body,
  onClose,
  onOpenRecord,
  projectId,
}: {
  body: CollectionBody
  onClose: () => void
  onOpenRecord: (id: string) => void
  projectId: string
}) {
  const [step, setStep] = useState<Step>({ kind: 'list' })
  if (step.kind === 'edit') return <TemplateEditor body={body} onBack={() => setStep({ kind: 'list' })} onClose={onClose} templateId={step.templateId} />
  if (step.kind === 'repeat') return <RepeatEditor name={step.name} onBack={() => setStep({ kind: 'list' })} onClose={onClose} templateId={step.templateId} />
  return (
    <TemplateList
      body={body}
      onClose={onClose}
      onEdit={(templateId) => setStep({ kind: 'edit', templateId })}
      onOpenRecord={onOpenRecord}
      onRepeat={(templateId, name) => setStep({ kind: 'repeat', templateId, name })}
      projectId={projectId}
    />
  )
}

function TemplateList({
  body,
  onClose,
  onEdit,
  onOpenRecord,
  onRepeat,
  projectId,
}: {
  body: CollectionBody
  onClose: () => void
  onEdit: (templateId: string | null) => void
  onOpenRecord: (id: string) => void
  onRepeat: (templateId: string, name: string) => void
  projectId: string
}) {
  const workspaceId = useWorkspaceId() ?? undefined
  const client = useQueryClient()
  const collection = body.collection.id
  const templates = useQuery(opQuery('record_template_list', { collection }, { workspaceId }))
  const recurring = useQuery(opQuery('recurring_record_list', { collection }, { workspaceId }))
  const make = useOp('record_create', {
    workspaceId,
    onSuccess: (made) => {
      void client.invalidateQueries({ queryKey: collectionKeys.allRecords(projectId, collection) })
      onClose()
      onOpenRecord(made.id)
    },
  })
  const archive = useOp('record_template_update', { workspaceId, invalidates: ['record_template_list', 'recurring_record_list'] })
  const change = useOp('recurring_record_update', { workspaceId, invalidates: ['recurring_record_list'] })
  const stop = useOp('recurring_record_delete', {
    workspaceId,
    invalidates: ['recurring_record_list'],
    onSuccess: () => showToast({ title: 'Schedule removed. The records it made stay.' }),
  })
  const error = make.error ?? archive.error ?? change.error ?? stop.error
  const list = templates.data?.templates ?? []
  const schedules = recurring.data?.recurring ?? []
  return (
    <Dialog
      description={`Records to start from in ${body.collection.name}, and schedules that add one every day, week or month.`}
      footer={
        <>
          <Button onClick={onClose}>Close</Button>
          {body.canBuild ? (
            <Button icon="plus" onClick={() => onEdit(null)} variant="primary">
              New template
            </Button>
          ) : null}
        </>
      }
      onClose={onClose}
      open
      title="Templates"
      width={560}
    >
      {templates.isPending ? (
        <ListSkeleton label="Loading templates" rows={2} />
      ) : templates.isError ? (
        <ErrorState error={templates.error} onRetry={() => void templates.refetch()} title="Templates didn’t load" />
      ) : list.length ? (
        <RowList label="Templates">
          {list.map((template) => (
            <Row
              actions={
                <>
                  <Button busy={make.isPending} onClick={() => make.mutate({ collection, template: template.id })} size="sm" variant="secondary">
                    New record
                  </Button>
                  <Menu
                    items={[
                      { label: 'Repeat on a schedule…', icon: 'clock', onSelect: () => onRepeat(template.id, template.name) },
                      ...(body.canBuild
                        ? [
                            { label: 'Edit template…', icon: 'pencil' as const, onSelect: () => onEdit(template.id) },
                            { separator: true as const },
                            {
                              label: 'Archive',
                              icon: 'archive' as const,
                              onSelect: () => archive.mutate({ template: template.id, archived: true, expectedVersion: template.version }),
                            },
                          ]
                        : []),
                    ]}
                    label={`${template.name} actions`}
                    trigger={<IconButton icon="more" label={`${template.name} actions`} size="sm" />}
                  />
                </>
              }
              key={template.id}
              meta={<span className="aw-row__metaline aw-wrap">{template.title}</span>}
              title={template.name}
            />
          ))}
        </RowList>
      ) : (
        <EmptyState icon="list" title="No templates yet">
          {body.canBuild ? 'Save the title and values a kind of record starts with, then add records from it or on a schedule.' : 'People who build this collection can add templates.'}
        </EmptyState>
      )}
      {schedules.length ? (
        <RowList label="Schedules">
          {schedules.map((schedule) => (
            <Row
              actions={
                <Menu
                  items={[
                    schedule.enabled
                      ? { label: 'Pause', icon: 'stop', onSelect: () => change.mutate({ recurring: schedule.id, enabled: false, expectedVersion: schedule.version }) }
                      : { label: 'Start again, as you', icon: 'play', onSelect: () => change.mutate({ recurring: schedule.id, enabled: true, expectedVersion: schedule.version }) },
                    { separator: true },
                    { label: 'Remove schedule', icon: 'trash', danger: true, onSelect: () => stop.mutate({ recurring: schedule.id }) },
                  ]}
                  label={`${schedule.templateName} schedule actions`}
                  trigger={<IconButton icon="more" label={`${schedule.templateName} schedule actions`} size="sm" />}
                />
              }
              className={schedule.enabled ? undefined : 'is-disabled'}
              key={schedule.id}
              meta={
                <span className="aw-row__metaline aw-wrap">
                  <span>{describeSchedule(schedule)}</span>
                  {schedule.ownerName ? <span>As {schedule.ownerName}</span> : null}
                  {schedule.pausedReason ? <span>{schedule.pausedReason}</span> : null}
                </span>
              }
              title={
                <span className="aw-row__titleline">
                  {schedule.templateName}
                  {schedule.enabled ? null : <StatusChip tone="muted">Paused</StatusChip>}
                </span>
              }
            />
          ))}
        </RowList>
      ) : null}
      {error ? <Banner tone="danger">{describeError(error).message}</Banner> : null}
    </Dialog>
  )
}

/** A template's name, title and values; a new one when `templateId` is null. */
function TemplateEditor({ body, onBack, onClose, templateId }: { body: CollectionBody; onBack: () => void; onClose: () => void; templateId: string | null }) {
  const workspaceId = useWorkspaceId() ?? undefined
  const templates = useQuery({ ...opQuery('record_template_list', { collection: body.collection.id }, { workspaceId }), enabled: templateId !== null })
  const saved = templateId ? templates.data?.templates.find((template) => template.id === templateId) : undefined
  if (templateId && !saved) {
    return (
      <Dialog footer={<Button onClick={onBack}>Back</Button>} onClose={onClose} open title="Edit template" width={560}>
        {templates.isError ? <ErrorState error={templates.error} title="The template didn’t load" /> : <ListSkeleton label="Loading the template" rows={3} />}
      </Dialog>
    )
  }
  return <TemplateForm body={body} key={saved?.id ?? 'new'} onBack={onBack} onClose={onClose} saved={saved} />
}

function TemplateForm({
  body,
  onBack,
  onClose,
  saved,
}: {
  body: CollectionBody
  onBack: () => void
  onClose: () => void
  saved?: { id: string; name: string; title: string; fields: Record<string, unknown>; version: number }
}) {
  const workspaceId = useWorkspaceId() ?? undefined
  // The fields a template can hold here, asked like a form's questions.
  const questions = normalizeQuestions(
    body.fields.filter((field) => !field.archived).map((field) => ({ ...field, required: false, options: field.options.filter((option) => !option.archived) })),
  )
  const [name, setName] = useState(saved?.name ?? `${body.collection.name} template`.slice(0, 80))
  const [title, setTitle] = useState(saved?.title ?? '')
  const [answers, setAnswers] = useState<Answers>(() => {
    const start: Answers = {}
    for (const [key, value] of Object.entries(saved?.fields ?? {})) {
      if (typeof value === 'string' || typeof value === 'boolean' || (Array.isArray(value) && value.every((entry) => typeof entry === 'string'))) start[key] = value as Answers[string]
      else if (typeof value === 'number') start[key] = String(value)
    }
    return start
  })
  const done = () => {
    showToast({ title: `${name.trim()} saved` })
    onBack()
  }
  const create = useOp('record_template_create', { workspaceId, invalidates: ['record_template_list'], onSuccess: done })
  const update = useOp('record_template_update', { workspaceId, invalidates: ['record_template_list'], onSuccess: done })
  const pending = create.isPending || update.isPending
  const error = create.error ?? update.error
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (pending || !name.trim() || !title.trim()) return
    const { title: _title, ...fields } = answerBody({ fields: questions }, { ...answers, title: title })
    void _title
    if (saved) update.mutate({ template: saved.id, name: name.trim(), title: title.trim(), fields, expectedVersion: saved.version })
    else create.mutate({ collection: body.collection.id, name: name.trim(), title: title.trim(), fields })
  }
  return (
    <Dialog
      footer={
        <>
          <Button onClick={onBack}>Back</Button>
          <Button busy={pending} disabled={!name.trim() || !title.trim()} type="submit" variant="primary">
            {saved ? 'Save template' : 'Make template'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title={saved ? `Edit ${saved.name}` : 'New template'}
      width={560}
    >
      <div className="aw-form">
        <TextField data-autofocus label="Template name" maxLength={80} onChange={(event) => setName(event.target.value)} required value={name} />
        <TextField
          hint="Write {date} where the day a record is made should go."
          label={`${body.collection.titleName} of each record`}
          maxLength={300}
          onChange={(event) => setTitle(event.target.value)}
          placeholder="Check-in {date}"
          required
          value={title}
        />
        <Questions answers={answers} fields={questions} onChange={(id, value) => setAnswers((current) => ({ ...current, [id]: value }))} />
        {error ? <Banner tone="danger">{describeError(error).message}</Banner> : null}
      </div>
    </Dialog>
  )
}

/** A schedule that makes a record from the template. */
function RepeatEditor({ name, onBack, onClose, templateId }: { name: string; onBack: () => void; onClose: () => void; templateId: string }) {
  const workspaceId = useWorkspaceId() ?? undefined
  const client = useQueryClient()
  const [draft, setDraft] = useState<ScheduleDraft>(() => ({
    frequency: 'weekly',
    every: '1',
    weekdays: [1],
    monthDay: '1',
    timeOfDay: '09:00',
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC',
  }))
  const create = useOp('recurring_record_create', {
    workspaceId,
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: opPrefix('recurring_record_list', workspaceId) })
      showToast({ title: `${name} set to repeat`, description: describeSchedule(scheduleInput(draft)) })
      onBack()
    },
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!create.isPending) create.mutate({ template: templateId, ...scheduleInput(draft) })
  }
  const set = (change: Partial<ScheduleDraft>) => setDraft((current) => ({ ...current, ...change }))
  return (
    <Dialog
      description="Each record is added as you. If you can no longer add records here, it pauses and says why."
      footer={
        <>
          <Button onClick={onBack}>Back</Button>
          <Button busy={create.isPending} type="submit" variant="primary">
            Repeat
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title={`Repeat ${name}`}
      width={520}
    >
      <div className="aw-form">
        <SegmentedControl<Frequency>
          label="How often"
          onChange={(frequency) => set({ frequency })}
          options={[
            { value: 'daily', label: 'Daily' },
            { value: 'weekly', label: 'Weekly' },
            { value: 'monthly', label: 'Monthly' },
          ]}
          value={draft.frequency}
        />
        <TextField
          inputMode="numeric"
          label={`Every how many ${draft.frequency === 'daily' ? 'days' : draft.frequency === 'weekly' ? 'weeks' : 'months'}`}
          max={52}
          min={1}
          onChange={(event) => set({ every: event.target.value })}
          type="number"
          value={draft.every}
        />
        {draft.frequency === 'weekly' ? (
          <fieldset className="aw-pform__group">
            <legend className="aw-pform__legend">On</legend>
            <div className="aw-cforms__values">
              {WEEKDAYS.map((day) => (
                <Checkbox
                  checked={draft.weekdays.includes(day.value)}
                  key={day.value}
                  label={day.label}
                  onChange={(checked) =>
                    set({ weekdays: checked ? [...draft.weekdays, day.value].sort((left, right) => left - right) : draft.weekdays.filter((entry) => entry !== day.value) })
                  }
                />
              ))}
            </div>
          </fieldset>
        ) : null}
        {draft.frequency === 'monthly' ? (
          <TextField
            hint="In a shorter month, its last day."
            inputMode="numeric"
            label="Day of the month"
            max={31}
            min={1}
            onChange={(event) => set({ monthDay: event.target.value })}
            type="number"
            value={draft.monthDay}
          />
        ) : null}
        <TextField label="At" onChange={(event) => set({ timeOfDay: event.target.value })} type="time" value={draft.timeOfDay} />
        <TextField hint="Such as Europe/Amsterdam." label="Time zone" onChange={(event) => set({ timezone: event.target.value })} value={draft.timezone} />
        {create.error ? <Banner tone="danger">{describeError(create.error).message}</Banner> : null}
      </div>
    </Dialog>
  )
}
