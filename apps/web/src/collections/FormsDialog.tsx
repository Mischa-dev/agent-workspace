import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp, opPrefix, opQuery, useOp } from '../api/ops'
import type { FormSummary } from '../api/generated/operations'
import { describeError } from '../app/mutations'
import { useWorkspaceId } from '../app/workspace'
import type { FieldDefinition } from '../fields/fieldModel'
import {
  bugReportKeys,
  createCollectionForm,
  deleteBugReportLink,
  listBugReportLinks,
  replaceBugReportLink,
  updateBugReportLink,
  type BugReportLink,
  type FormSettings,
} from '../settings/project/bugReports'
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
  Row,
  RowList,
  SegmentedControl,
  SelectField,
  StatusChip,
  Switch,
  TextArea,
  TextField,
  countLabel,
  formatRelative,
  showToast,
} from '../ui'
import type { CollectionBody } from './api'
import {
  askableFields,
  canBeCondition,
  conditionValues,
  dailyLimitProblem,
  formDraft,
  formFromDraft,
  MAX_DAILY_LIMIT,
  type FormAudience,
  type FormDraft,
} from './formSettings'
import { FormFill } from './FormFill'
import '../styles/pages.css'

type Step =
  | { kind: 'list' }
  | { kind: 'edit'; link: BugReportLink | null }
  | { kind: 'edit-form'; formId: string }
  | { kind: 'fill'; formId: string }
  | { kind: 'shared'; name: string; url: string; replaced: boolean }

/**
 * A collection's forms (AWD-110): public pages anyone with the link fills in,
 * each answer a new record. They're bug report links underneath, so the
 * secret URL is shown once and replacing it cuts the old one off. Internal
 * forms (T1.28) are filled in here by the space's members.
 */
export function FormsDialog({ body, onClose, projectId }: { body: CollectionBody; onClose: () => void; projectId: string }) {
  const [step, setStep] = useState<Step>({ kind: 'list' })
  if (step.kind === 'fill') {
    return <FormFill formId={step.formId} onBack={() => setStep({ kind: 'list' })} onClose={onClose} projectId={projectId} />
  }
  if (step.kind === 'edit-form') {
    return <InternalFormEditor body={body} formId={step.formId} onBack={() => setStep({ kind: 'list' })} onClose={onClose} projectId={projectId} />
  }
  if (step.kind === 'edit') {
    return (
      <FormEditor
        body={body}
        link={step.link}
        onBack={() => setStep({ kind: 'list' })}
        onClose={onClose}
        onShared={(name, url) => setStep({ kind: 'shared', name, url, replaced: false })}
        projectId={projectId}
      />
    )
  }
  if (step.kind === 'shared') {
    return <SharedForm name={step.name} onDone={() => setStep({ kind: 'list' })} onClose={onClose} replaced={step.replaced} url={step.url} />
  }
  return (
    <FormList
      body={body}
      onClose={onClose}
      onEdit={(link) => setStep({ kind: 'edit', link })}
      onEditForm={(formId) => setStep({ kind: 'edit-form', formId })}
      onFill={(formId) => setStep({ kind: 'fill', formId })}
      onReplaced={(name, url) => setStep({ kind: 'shared', name, url, replaced: true })}
      projectId={projectId}
    />
  )
}

function FormList({
  body,
  onClose,
  onEdit,
  onEditForm,
  onFill,
  onReplaced,
  projectId,
}: {
  body: CollectionBody
  onClose: () => void
  onEdit: (link: BugReportLink | null) => void
  onEditForm: (formId: string) => void
  onFill: (formId: string) => void
  onReplaced: (name: string, url: string) => void
  projectId: string
}) {
  const client = useQueryClient()
  const workspaceId = useWorkspaceId() ?? undefined
  const list = useQuery({ queryKey: bugReportKeys.links(projectId), queryFn: () => listBugReportLinks(projectId) })
  const internal = useQuery(opQuery('form_list', { spaceId: projectId, collection: body.collection.id }, { workspaceId }))
  const archive = useOp('form_update', {
    workspaceId,
    invalidates: ['form_list'],
    onSuccess: (form) => showToast({ title: `${form.name} archived` }),
  })
  const internalForms: FormSummary[] = (internal.data?.forms ?? []).filter((form) => form.audience === 'internal')
  const refresh = () => client.invalidateQueries({ queryKey: bugReportKeys.links(projectId) })
  const [problem, setProblem] = useState<string | null>(null)
  const [deleting, setDeleting] = useState<BugReportLink | null>(null)
  const change = useMutation({
    mutationFn: async ({ link, action }: { link: BugReportLink; action: 'toggle' | 'replace' | 'delete' }) => {
      if (action === 'toggle') return updateBugReportLink(link, { state: link.state === 'active' ? 'paused' : 'active' })
      if (action === 'replace') {
        const result = await replaceBugReportLink(link)
        onReplaced(result.link.name, result.formUrl ?? result.url)
        return result.link
      }
      await deleteBugReportLink(link)
      return null
    },
    onMutate: () => setProblem(null),
    onSuccess: (saved, { link, action }) => {
      setDeleting(null)
      if (action === 'delete') showToast({ title: `${link.name} deleted` })
      else if (action === 'toggle' && saved) showToast({ title: saved.state === 'active' ? `${saved.name} is taking answers` : `${saved.name} paused` })
    },
    onError: (error, { link }) => setProblem(`${link.name} didn’t change. ${describeError(error).message}`),
    onSettled: refresh,
  })
  const forms = list.data?.links.filter((link) => link.collectionId === body.collection.id) ?? []
  // Forms need record-edit permission on the collection, not task rights.
  const canMake = body.canEditRecords

  return (
    <Dialog
      description={`A public form’s link lets anyone add a record to ${body.collection.name} without signing in, as the person who shared it. An internal form is filled in here by people in this project, as themselves.`}
      footer={
        <>
          <Button onClick={onClose}>Close</Button>
          {canMake ? (
            <Button icon="plus" onClick={() => onEdit(null)} variant="primary">
              New form
            </Button>
          ) : null}
        </>
      }
      onClose={onClose}
      open
      title="Forms"
      width={560}
    >
      {list.isPending ? (
        <ListSkeleton label="Loading forms" rows={2} />
      ) : list.isError ? (
        <ErrorState error={list.error} onRetry={() => void list.refetch()} title="Forms didn’t load" />
      ) : forms.length || internalForms.length ? (
        <RowList label="Forms">
          {internalForms.map((form) => (
            <Row
              actions={
                <>
                  <Button onClick={() => onFill(form.id)} size="sm" variant="secondary">
                    Fill in
                  </Button>
                  {body.canBuild ? (
                    <Menu
                      items={[
                        { label: 'Edit form…', icon: 'pencil', onSelect: () => onEditForm(form.id) },
                        { separator: true },
                        {
                          label: 'Archive',
                          icon: 'archive',
                          onSelect: () => archive.mutate({ form: form.id, archived: true, expectedVersion: form.version }),
                        },
                      ]}
                      label={`${form.name} actions`}
                      trigger={<IconButton icon="more" label={`${form.name} actions`} size="sm" />}
                    />
                  ) : null}
                </>
              }
              key={form.id}
              meta={
                <span className="aw-row__metaline aw-wrap">
                  <span>{countLabel(form.questions, 'question')}</span>
                  <span>Filled in by people in this project</span>
                </span>
              }
              title={
                <span className="aw-row__titleline">
                  {form.name}
                  <StatusChip tone="muted">Internal</StatusChip>
                </span>
              }
            />
          ))}
          {forms.map((link) => (
            <Row
              actions={
                link.canManage ? (
                  <Menu
                    items={[
                      { label: 'Edit form…', icon: 'pencil', onSelect: () => onEdit(link) },
                      {
                        label: link.state === 'active' ? 'Pause' : 'Take answers again',
                        icon: link.state === 'active' ? 'stop' : 'play',
                        onSelect: () => change.mutate({ link, action: 'toggle' }),
                      },
                      { label: 'Replace the link', icon: 'key', onSelect: () => change.mutate({ link, action: 'replace' }) },
                      { separator: true },
                      { label: 'Delete', icon: 'trash', danger: true, onSelect: () => setDeleting(link) },
                    ]}
                    label={`${link.name} actions`}
                    trigger={<IconButton icon="more" label={`${link.name} actions`} size="sm" />}
                  />
                ) : null
              }
              className={link.state === 'active' ? undefined : 'is-disabled'}
              key={link.id}
              meta={
                <span className="aw-row__metaline aw-wrap">
                  <span>{countLabel(link.reportCount, 'answer')}</span>
                  <span>{link.lastReportAt ? `Last ${formatRelative(link.lastReportAt)}` : 'No answers yet'}</span>
                  {link.ownerName ? <span>Shared by {link.ownerName}</span> : null}
                  {link.disabledReason ? <span>{link.disabledReason}</span> : null}
                </span>
              }
              title={
                <span className="aw-row__titleline">
                  {link.name}
                  {link.state === 'active' ? null : <StatusChip tone={link.state === 'paused' ? 'muted' : 'danger'}>{link.state === 'paused' ? 'Paused' : 'Turned off'}</StatusChip>}
                </span>
              }
            />
          ))}
        </RowList>
      ) : (
        <EmptyState icon="list" title="No forms yet">
          {canMake
            ? 'Make one to collect requests, sign-ups or reports, from people outside this project or inside it.'
            : 'People who can add records here can share a form.'}
        </EmptyState>
      )}
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      {archive.error ? <Banner tone="danger">{`The form wasn’t archived. ${describeError(archive.error).message}`}</Banner> : null}
      {deleting ? (
        <div className="aw-cforms__confirm" role="group" aria-label={`Delete ${deleting.name}?`}>
          <p>Delete {deleting.name}? Its link stops working right away. Records it added stay.</p>
          <Button onClick={() => setDeleting(null)} size="sm">
            Keep it
          </Button>
          <Button busy={change.isPending} onClick={() => change.mutate({ link: deleting, action: 'delete' })} size="sm" variant="danger">
            Delete form
          </Button>
        </div>
      ) : null}
    </Dialog>
  )
}

/** An internal form's editor, once its settings have loaded. */
function InternalFormEditor({
  body,
  formId,
  onBack,
  onClose,
  projectId,
}: {
  body: CollectionBody
  formId: string
  onBack: () => void
  onClose: () => void
  projectId: string
}) {
  const workspaceId = useWorkspaceId() ?? undefined
  const form = useQuery(opQuery('form_get', { form: formId }, { workspaceId }))
  if (!form.data) {
    return (
      <Dialog footer={<Button onClick={onBack}>Back</Button>} onClose={onClose} open title="Edit form" width={560}>
        {form.isError ? <ErrorState error={form.error} onRetry={() => void form.refetch()} title="The form didn’t load" /> : <ListSkeleton label="Loading the form" rows={3} />}
      </Dialog>
    )
  }
  return (
    <FormEditor
      body={body}
      internal={{ id: form.data.id, version: form.data.version, name: form.data.name, form: form.data.definition as FormSettings }}
      link={null}
      onBack={onBack}
      onClose={onClose}
      onShared={() => onBack()}
      projectId={projectId}
    />
  )
}

function FormEditor({
  body,
  internal,
  link,
  onBack,
  onClose,
  onShared,
  projectId,
}: {
  body: CollectionBody
  /** An internal form being edited (T1.28). */
  internal?: { id: string; version: number; name: string; form: FormSettings }
  link: BugReportLink | null
  onBack: () => void
  onClose: () => void
  onShared: (name: string, url: string) => void
  projectId: string
}) {
  const client = useQueryClient()
  const workspaceId = useWorkspaceId() ?? undefined
  const editing = Boolean(link || internal)
  const [draft, setDraft] = useState<FormDraft>(() =>
    formDraft(link ?? internal ?? null, body.collection.name, askableFields(body.fields, internal ? 'internal' : 'public'), internal ? 'internal' : 'public'),
  )
  const audience: FormAudience = draft.audience ?? 'public'
  const fields = askableFields(body.fields, audience)
  const [initial] = useState(draft)
  const [error, setError] = useState<string | null>(null)
  const save = useMutation({
    mutationFn: async () => {
      const form: FormSettings = formFromDraft(draft, fields)
      const name = draft.name.trim()
      if (internal) {
        await callOp('form_update', { form: internal.id, name, definition: form }, { workspaceId, expectedVersion: internal.version })
        return null
      }
      if (audience === 'internal') {
        await callOp('form_create', { collection: body.collection.id, name, definition: form }, { workspaceId })
        return null
      }
      if (link) {
        await updateBugReportLink(link, { name, form })
        return null
      }
      return createCollectionForm(projectId, body.collection.id, name, form)
    },
    // A link made on an earlier try can answer without its address: list it either way.
    onSettled: () => {
      void client.invalidateQueries({ queryKey: bugReportKeys.links(projectId) })
      void client.invalidateQueries({ queryKey: opPrefix('form_list', workspaceId) })
      void client.invalidateQueries({ queryKey: opPrefix('form_get', workspaceId) })
    },
    onSuccess: (created) => {
      if (created) onShared(created.link.name, created.formUrl ?? created.url)
      else {
        showToast({ title: `${draft.name.trim()} saved` })
        onBack()
      }
    },
    onError: (reason) => setError(`The form wasn’t saved. ${describeError(reason).message}`),
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const problem = audience === 'public' ? dailyLimitProblem(draft) : null
    if (problem) {
      setError(problem)
      return
    }
    if (draft.name.trim()) save.mutate()
  }
  const toggle = (field: FieldDefinition, asked: boolean) =>
    setDraft((current) => ({ ...current, asked: { ...current.asked, [field.id]: asked ? (current.asked[field.id] ?? 'optional') : undefined } }))
  return (
    <Dialog
      dirty={JSON.stringify(draft) !== JSON.stringify(initial)}
      footer={
        <>
          <Button onClick={onBack}>Back</Button>
          <Button busy={save.isPending} disabled={!draft.name.trim()} type="submit" variant="primary">
            {editing ? 'Save form' : 'Make form'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title={link ? `Edit ${link.name}` : internal ? `Edit ${internal.name}` : 'New form'}
      width={560}
    >
      <div className="aw-form">
        <TextField label="Form name" maxLength={80} onChange={(event) => setDraft({ ...draft, name: event.target.value })} required value={draft.name} />
        {editing ? null : (
          <SegmentedControl<FormAudience>
            label="Who fills it in"
            onChange={(next) =>
              setDraft((current) => {
                // People questions stay inside the project: a public form drops them.
                const allowed = new Set(askableFields(body.fields, next).map((field) => field.id))
                const asked = Object.fromEntries(Object.entries(current.asked).filter(([id]) => allowed.has(id)))
                return { ...current, audience: next, asked }
              })
            }
            options={[
              { value: 'public', label: 'Anyone with the link' },
              { value: 'internal', label: 'People in this project' },
            ]}
            value={audience}
          />
        )}
        <TextArea
          hint="Shown above the questions."
          label="Intro"
          maxLength={2000}
          onChange={(event) => setDraft({ ...draft, intro: event.target.value })}
          rows={2}
          value={draft.intro}
        />
        <fieldset className="aw-cforms__fields">
          <legend>Questions</legend>
          <p className="aw-cforms__hint">{body.collection.titleName} is always asked. Pick the fields people fill in after it.</p>
          {fields.length ? (
            <ul className="aw-cforms__list">
              {fields.map((field) => {
                const asked = draft.asked[field.id]
                return (
                  <li className="aw-cforms__field" key={field.id}>
                    <Checkbox checked={asked !== undefined} label={field.name} onChange={(checked) => toggle(field, checked)} />
                    {asked !== undefined && field.type !== 'checkbox' ? (
                      <Switch
                        checked={asked === 'required'}
                        label={
                          <>
                            Required<span className="aw-sr-only"> ({field.name})</span>
                          </>
                        }
                        onChange={(required) => setDraft((current) => ({ ...current, asked: { ...current.asked, [field.id]: required ? 'required' : 'optional' } }))}
                      />
                    ) : null}
                    {asked !== undefined ? <QuestionLogic draft={draft} field={field} fields={fields} setDraft={setDraft} /> : null}
                  </li>
                )
              })}
            </ul>
          ) : (
            <p className="aw-cforms__hint">This collection has no fields a form can ask yet; people fields and links stay inside the project.</p>
          )}
        </fieldset>
        <TextArea
          hint="Shown once someone sends their answers."
          label="Thank-you note"
          maxLength={500}
          onChange={(event) => setDraft({ ...draft, thanks: event.target.value })}
          rows={2}
          value={draft.thanks}
        />
        {audience === 'public' ? (
          <TextField
            hint="When it takes this many answers in a day, the form pauses and tells you. Turn it on again to take more."
            inputMode="numeric"
            label="Answers a day"
            max={MAX_DAILY_LIMIT}
            min={1}
            onChange={(event) => setDraft({ ...draft, dailyLimit: event.target.value })}
            type="number"
            value={draft.dailyLimit}
          />
        ) : null}
        {error ? <Banner tone="danger">{error}</Banner> : null}
      </div>
    </Dialog>
  )
}

/**
 * When a question shows (T1.28): always, or only after an earlier choice or
 * checkbox is answered with one of some values; and a heading it starts.
 */
function QuestionLogic({
  draft,
  field,
  fields,
  setDraft,
}: {
  draft: FormDraft
  field: FieldDefinition
  fields: FieldDefinition[]
  setDraft: (change: (current: FormDraft) => FormDraft) => void
}) {
  const index = fields.findIndex((candidate) => candidate.id === field.id)
  const earlier = fields.slice(0, index).filter((candidate) => draft.asked[candidate.id] !== undefined && canBeCondition(candidate))
  const condition = draft.conditions?.[field.id]
  const on = earlier.find((candidate) => candidate.id === condition?.on)
  const setCondition = (next: { on: string; values: string[] } | undefined) =>
    setDraft((current) => ({ ...current, conditions: { ...current.conditions, [field.id]: next } }))
  return (
    <div className="aw-cforms__logic">
      {earlier.length ? (
        <SelectField
          label={
            <>
              Show<span className="aw-sr-only"> {field.name}</span>
            </>
          }
          onChange={(event) => setCondition(event.target.value ? { on: event.target.value, values: [] } : undefined)}
          value={on?.id ?? ''}
        >
          <option value="">Always</option>
          {earlier.map((candidate) => (
            <option key={candidate.id} value={candidate.id}>
              When {candidate.name} is…
            </option>
          ))}
        </SelectField>
      ) : null}
      {on && condition ? (
        <div className="aw-cforms__values" role="group" aria-label={`${on.name} values that show ${field.name}`}>
          {conditionValues(on).map((value) => (
            <Checkbox
              checked={condition.values.includes(value)}
              key={value}
              label={value}
              onChange={(checked) => setCondition({ on: on.id, values: checked ? [...condition.values, value] : condition.values.filter((entry) => entry !== value) })}
            />
          ))}
        </div>
      ) : null}
      <TextField
        label={
          <>
            Section heading<span className="aw-sr-only"> before {field.name}</span>
          </>
        }
        maxLength={120}
        onChange={(event) => setDraft((current) => ({ ...current, sections: { ...current.sections, [field.id]: event.target.value } }))}
        placeholder="None"
        value={draft.sections?.[field.id] ?? ''}
      />
    </div>
  )
}

/** The form's link, shown once: it's a secret anyone can add records with. */
function SharedForm({ name, onClose, onDone, replaced, url }: { name: string; onClose: () => void; onDone: () => void; replaced: boolean; url: string }) {
  const [copied, setCopied] = useState<'yes' | 'failed' | null>(null)
  const timer = useRef<number | undefined>(undefined)
  useEffect(() => () => window.clearTimeout(timer.current), [])
  const copy = () => {
    if (!navigator.clipboard) {
      setCopied('failed')
      return
    }
    navigator.clipboard.writeText(url).then(
      () => {
        setCopied('yes')
        window.clearTimeout(timer.current)
        timer.current = window.setTimeout(() => setCopied(null), 2000)
      },
      () => setCopied('failed'),
    )
  }
  return (
    <Dialog
      footer={
        <>
          <a className="aw-btn aw-btn--secondary aw-btn--md" href={url} rel="noreferrer" target="_blank">
            <Icon name="external" size={16} />
            <span className="aw-btn__label">Open form</span>
          </a>
          <Button icon={copied === 'yes' ? 'check' : 'copy'} onClick={copy} variant="primary">
            {copied === 'yes' ? 'Copied' : 'Copy link'}
          </Button>
        </>
      }
      onClose={() => {
        onDone()
        onClose()
      }}
      open
      title={replaced ? `${name} has a new link` : `${name} is ready`}
    >
      <div className="aw-form">
        <p className="aw-cforms__hint">
          {replaced ? 'The old link stopped working. ' : ''}Anyone with this link can add records, so share it with the people it’s for. It can’t be shown again; replace it if
          you lose it.
        </p>
        <code className="aw-token__value">{url}</code>
        {copied === 'failed' ? <Banner tone="warning">The link wasn’t copied. Select it and copy it yourself.</Banner> : null}
        <Button onClick={onDone} variant="ghost">
          Back to forms
        </Button>
      </div>
    </Dialog>
  )
}
