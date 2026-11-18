import { useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { opQuery, useOp } from '../api/ops'
import { useWorkspaceId } from '../app/workspace'
import { projectAreaHref, projectLocationHref } from '../projects/locations'
import { useShellPage } from '../shell/shellStore'
import { Banner, Button, Dialog, EmptyState, ErrorState, ListSkeleton, PageHeader, StatusChip, TextField, describeError, type ChipTone } from '../ui'
import { formatDateLabel } from '../ui/dateModel'
import { fieldText, initiativeProgress, share, timeline, type Initiative, type InitiativeMilestone } from './initiativeModel'
import './initiatives.css'

/** How a Health or Status value reads as a chip. */
function toneOf(value: string): ChipTone {
  const word = value.toLowerCase()
  if (word === 'on track' || word === 'done') return 'success'
  if (word === 'at risk') return 'warning'
  if (word === 'off track' || word === 'canceled') return 'danger'
  return 'muted'
}

/**
 * Initiatives (T1.26): work that spans projects, each followed through the
 * milestones its projects put in it. Counts take in only the projects you
 * can see.
 */
export function InitiativesPage() {
  const workspaceId = useWorkspaceId() ?? undefined
  const list = useQuery({ ...opQuery('initiative_list', {}, { workspaceId }), staleTime: 30_000 })
  const [starting, setStarting] = useState(false)
  useShellPage({ home: true, mainClassName: 'aw-main--page' })
  const data = list.data
  const initiatives = data?.initiatives ?? []
  const recordHref = (initiative: Initiative) =>
    data?.spaceId && data.collectionId ? projectAreaHref(data.spaceId, 'collections', data.collectionId, { record: initiative.id }) : null

  return (
    <div className="aw-page aw-initiatives">
      <PageHeader
        actions={
          <Button icon="plus" onClick={() => setStarting(true)} variant="primary">
            New initiative
          </Button>
        }
        icon="branch"
        title="Initiatives"
      />
      {list.isPending ? <ListSkeleton label="Loading initiatives" rows={4} /> : null}
      {list.isError ? <ErrorState error={list.error} onRetry={() => void list.refetch()} title="Initiatives didn’t load" /> : null}
      {data && initiatives.length === 0 ? (
        <EmptyState
          action={
            <Button onClick={() => setStarting(true)} variant="primary">
              Start an initiative
            </Button>
          }
          icon="branch"
          title="No initiatives yet"
        >
          An initiative follows work that spans projects. Each project adds its milestones to it, and its progress counts their tasks.
        </EmptyState>
      ) : null}
      {initiatives.length > 0 ? (
        <ul className="aw-initiatives__list">
          {initiatives.map((initiative) => (
            <InitiativeCard href={recordHref(initiative)} initiative={initiative} key={initiative.id} />
          ))}
        </ul>
      ) : null}
      {data?.collectionId && data.spaceId ? (
        <p className="aw-initiatives__more">
          Initiatives are records of the workspace’s{' '}
          <Link to={projectAreaHref(data.spaceId, 'collections', data.collectionId)}>Initiatives collection</Link>, where you can change their fields
          and views.
        </p>
      ) : null}
      {starting ? <StartDialog onClose={() => setStarting(false)} workspaceId={workspaceId} /> : null}
    </div>
  )
}

function InitiativeCard({ href, initiative }: { href: string | null; initiative: Initiative }) {
  const headingId = `initiative-${initiative.id}`
  const progress = share(initiative)
  return (
    <li aria-labelledby={headingId} className={`aw-initiative${initiative.archived ? ' is-archived' : ''}`}>
      <div className="aw-initiative__head">
        <h2 className="aw-initiative__title" id={headingId}>
          {href ? <Link to={href}>{initiative.title}</Link> : initiative.title}
        </h2>
        <ul aria-label="Fields" className="aw-initiative__fields">
          {initiative.fields.map((field) => {
            const text = fieldText(field.value)
            if (!text) return null
            const chip = field.type === 'status' || field.type === 'single_choice'
            const shown = field.type === 'date' ? formatDateLabel(text) : text
            return (
              <li key={field.name}>
                {chip ? (
                  <StatusChip tone={toneOf(text)}>
                    <span className="aw-sr-only">{field.name}: </span>
                    {shown}
                  </StatusChip>
                ) : (
                  <span className="aw-initiative__field">
                    <span className="aw-initiative__field-name">{field.name}</span> {shown}
                  </span>
                )}
              </li>
            )
          })}
        </ul>
      </div>
      <span className="aw-progress aw-initiative__progress">
        <progress aria-label={`${initiative.title} progress`} className="aw-progress__bar" max={100} value={Math.round(progress * 100)} />
        <span className="aw-progress__text">{initiativeProgress(initiative)}</span>
      </span>
      {initiative.milestones.length === 0 ? (
        <p className="aw-initiative__empty">No milestones yet. Add one from a project’s milestones, under Edit.</p>
      ) : (
        <>
          <Timeline milestones={initiative.milestones} title={initiative.title} />
          <ul aria-label={`${initiative.title} milestones`} className="aw-initiative__milestones">
            {initiative.milestones.map((milestone) => (
              <li key={milestone.id}>
                <Link to={projectLocationHref(milestone.spaceId, { kind: 'milestones' })}>{milestone.name}</Link>
                <span className="aw-initiative__space">{milestone.spaceName}</span>
                <span className="aw-initiative__date">{milestone.targetDate ? formatDateLabel(milestone.targetDate) : 'No target date'}</span>
                <span className="aw-initiative__count">{initiativeProgress(milestone)}</span>
              </li>
            ))}
          </ul>
        </>
      )}
    </li>
  )
}

/** The milestones along one line by target date, with today marked. */
function Timeline({ milestones, title }: { milestones: InitiativeMilestone[]; title: string }) {
  const laid = timeline(milestones)
  if (laid.points.length === 0) return null
  return (
    <div aria-hidden="true" className="aw-initiative__timeline" title={`${title} by target date`}>
      <span className="aw-initiative__track" />
      {laid.today !== null ? <span className="aw-initiative__today" style={{ left: `${laid.today * 100}%` }} title="Today" /> : null}
      {laid.points.map((point) => (
        <span
          className={`aw-initiative__point${point.milestone.scope > 0 && point.milestone.completed === point.milestone.scope ? ' is-done' : ''}`}
          key={point.milestone.id}
          style={{ left: `${point.at * 100}%` }}
          title={`${point.milestone.name} (${point.milestone.spaceKey}), ${formatDateLabel(point.milestone.targetDate)}: ${initiativeProgress(point.milestone)}`}
        />
      ))}
    </div>
  )
}

function StartDialog({ onClose, workspaceId }: { onClose: () => void; workspaceId?: string }) {
  const [title, setTitle] = useState('')
  const start = useOp('initiative_create', { workspaceId, invalidates: ['initiative_list'], onSuccess: onClose })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!title.trim() || start.isPending) return
    start.mutate({ title: title.trim() })
  }
  return (
    <Dialog
      dirty={Boolean(title)}
      footer={
        <>
          <Button onClick={onClose} variant="secondary">
            Cancel
          </Button>
          <Button busy={start.isPending} disabled={!title.trim()} type="submit" variant="primary">
            Start initiative
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title="New initiative"
    >
      <TextField
        data-autofocus
        hint="Projects add their milestones to it."
        label="Name"
        maxLength={300}
        onChange={(event) => setTitle(event.target.value)}
        required
        value={title}
      />
      {start.error ? <Banner tone="danger">{describeError(start.error).message}</Banner> : null}
    </Dialog>
  )
}
