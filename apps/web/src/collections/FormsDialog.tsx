import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { describeError } from '../app/mutations'
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
  StatusChip,
  Switch,
  TextArea,
  TextField,
  countLabel,
  formatRelative,
  showToast,
} from '../ui'
import type { CollectionBody } from './api'
import { askableFields, formDraft, formFromDraft, type FormDraft } from './formSettings'

type Step = { kind: 'list' } | { kind: 'edit'; link: BugReportLink | null } | { kind: 'shared'; name: string; url: string; replaced: boolean }

/**
 * A collection's forms (AWD-110): public pages anyone with the link fills in,
 * each answer a new record. They're bug report links underneath, so the
 * secret URL is shown once and replacing it cuts the old one off.
 */
export function FormsDialog({ body, onClose, projectId }: { body: CollectionBody; onClose: () => void; projectId: string }) {
  const [step, setStep] = useState<Step>({ kind: 'list' })
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
      onReplaced={(name, url) => setStep({ kind: 'shared', name, url, replaced: true })}
      projectId={projectId}
    />
  )
}

function FormList({
  body,
  onClose,
  onEdit,
  onReplaced,
  projectId,
}: {
  body: CollectionBody
  onClose: () => void
  onEdit: (link: BugReportLink | null) => void
  onReplaced: (name: string, url: string) => void
  projectId: string
}) {
  const client = useQueryClient()
  const list = useQuery({ queryKey: bugReportKeys.links(projectId), queryFn: () => listBugReportLinks(projectId) })
  const refresh = () => client.invalidateQueries({ queryKey: bugReportKeys.links(projectId) })
  const [problem, setProblem] = useState<string | null>(null)
  const [deleting, setDeleting] = useState<BugReportLink | null>(null)
  const change = useMutation({
    mutationFn: async ({ link, action }: { link: BugReportLink; action: 'toggle' | 'replace' | 'delete' }) => {
      if (action === 'toggle') return updateBugReportLink(link.id, { state: link.state === 'active' ? 'paused' : 'active' })
      if (action === 'replace') {
        const result = await replaceBugReportLink(link.id)
        onReplaced(result.link.name, result.formUrl ?? result.url)
        return result.link
      }
      await deleteBugReportLink(link.id)
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
  const canMake = body.canEditRecords && list.data?.canCreate === true

  return (
    <Dialog
      description={`Anyone with a form’s link can add a record to ${body.collection.name}, without signing in. Records are added as the person who shared the form.`}
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
      ) : forms.length ? (
        <RowList label="Forms">
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
          {canMake ? 'Make one to collect requests, sign-ups or reports from people outside this project.' : 'People who can add records here can share a form.'}
        </EmptyState>
      )}
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
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

function FormEditor({
  body,
  link,
  onBack,
  onClose,
  onShared,
  projectId,
}: {
  body: CollectionBody
  link: BugReportLink | null
  onBack: () => void
  onClose: () => void
  onShared: (name: string, url: string) => void
  projectId: string
}) {
  const client = useQueryClient()
  const fields = askableFields(body.fields)
  const [draft, setDraft] = useState<FormDraft>(() => formDraft(link, body.collection.name, fields))
  const [initial] = useState(draft)
  const [error, setError] = useState<string | null>(null)
  const save = useMutation({
    mutationFn: async () => {
      const form: FormSettings = formFromDraft(draft, fields)
      if (link) {
        await updateBugReportLink(link.id, { name: draft.name.trim(), form })
        return null
      }
      return createCollectionForm(projectId, body.collection.id, draft.name.trim(), form)
    },
    onSuccess: (created) => {
      void client.invalidateQueries({ queryKey: bugReportKeys.links(projectId) })
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
            {link ? 'Save form' : 'Make form'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title={link ? `Edit ${link.name}` : 'New form'}
      width={560}
    >
      <div className="aw-form">
        <TextField label="Form name" maxLength={80} onChange={(event) => setDraft({ ...draft, name: event.target.value })} required value={draft.name} />
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
        {error ? <Banner tone="danger">{error}</Banner> : null}
      </div>
    </Dialog>
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
