import { useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'

import { opQuery, useOp } from '../api/ops'
import { useWorkspaceId } from '../app/workspace'
import { Banner, Button, Dialog, SegmentedControl, Section, StatusChip, TextArea, TextField, describeError, showToast } from '../ui'
import { DatePicker, formatDateLabel } from '../ui/pickers'
import { HEALTHS, healthTone, type Health } from './projectStatusModel'
import { UpdateLine } from './UpdateLine'
import './projectStatus.css'

/**
 * A project's status on its home (T1.43): its health, target date and lead,
 * its latest update, and a way to post the next one.
 */
export default function ProjectStatus({ projectId }: { projectId: string }) {
  const workspaceId = useWorkspaceId() ?? undefined
  const status = useQuery({ ...opQuery('project_status_get', { spaceId: projectId }, { workspaceId }), staleTime: 30_000 })
  const setStatus = useOp('project_status_set', { workspaceId, invalidates: ['project_status_get'] })
  const [posting, setPosting] = useState(false)
  const data = status.data
  if (!data) return null
  const canUpdate = Boolean(data.canUpdate)
  if (!canUpdate && !data.health && !data.targetDate && !data.lead && !data.latestUpdate) return null
  const setTarget = (targetDate: string | null) =>
    setStatus.mutate(
      { spaceId: projectId, targetDate: targetDate ?? '', expectedVersion: data.version },
      { onError: (error) => showToast({ title: 'The target date didn’t change', description: describeError(error).message, tone: 'danger' }) },
    )

  return (
    <Section
      actions={
        canUpdate ? (
          <Button icon="plus" onClick={() => setPosting(true)} size="sm" variant="secondary">
            Post update
          </Button>
        ) : null
      }
      id="overview-status"
      title="Status"
    >
      <div className="aw-project-status">
        <div className="aw-project-status__facts">
          <StatusChip tone={healthTone(data.health)}>{data.healthLabel ?? 'No health yet'}</StatusChip>
          {canUpdate ? (
            <DatePicker disabled={setStatus.isPending} label="Target date" onChange={setTarget} placeholder="No target date" prefix="Target" value={data.targetDate ?? null} />
          ) : (
            <span className="aw-project-status__fact">{data.targetDate ? `Target ${formatDateLabel(data.targetDate)}` : 'No target date'}</span>
          )}
          {data.lead ? <span className="aw-project-status__fact">Lead: {data.lead.name}</span> : null}
        </div>
        {data.latestUpdate ? (
          <UpdateLine showHealth={false} spaceId={projectId} update={data.latestUpdate} />
        ) : (
          <p className="aw-project-status__empty">No updates yet. An update says how the project is going and sets its health.</p>
        )}
      </div>
      {posting ? <PostUpdateDialog current={data.health as Health | undefined} onClose={() => setPosting(false)} projectId={projectId} workspaceId={workspaceId} /> : null}
    </Section>
  )
}

function PostUpdateDialog({
  current,
  onClose,
  projectId,
  workspaceId,
}: {
  current?: Health
  onClose: () => void
  projectId: string
  workspaceId?: string
}) {
  const [health, setHealth] = useState<Health>(current ?? 'on_track')
  const [title, setTitle] = useState('')
  const [text, setText] = useState('')
  const post = useOp('project_update_create', {
    workspaceId,
    invalidates: ['project_status_get', 'initiative_list'],
    onSuccess: () => {
      showToast({ title: 'Update posted', tone: 'success' })
      onClose()
    },
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!text.trim() || post.isPending) return
    post.mutate({ spaceId: projectId, health, text: text.trim(), ...(title.trim() ? { title: title.trim() } : {}) })
  }
  return (
    <Dialog
      dirty={Boolean(text || title)}
      footer={
        <>
          <Button onClick={onClose} variant="secondary">
            Cancel
          </Button>
          <Button busy={post.isPending} disabled={!text.trim()} type="submit" variant="primary">
            Post update
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title="Post an update"
    >
      <SegmentedControl<Health> label="How it’s going" onChange={setHealth} options={HEALTHS.map((entry) => ({ ...entry }))} value={health} />
      <TextArea
        data-autofocus
        hint="What moved, what’s next, what’s in the way. It goes in a thread in the project’s chat."
        label="Update"
        maxLength={8000}
        onChange={(event) => setText(event.target.value)}
        required
        rows={6}
        value={text}
      />
      <TextField label="Title" maxLength={120} onChange={(event) => setTitle(event.target.value)} placeholder="Update, with today’s date" value={title} />
      {post.error ? <Banner tone="danger">{describeError(post.error).message}</Banner> : null}
    </Dialog>
  )
}
