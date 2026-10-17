import { useEffect, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { describeError } from '../../app/mutations'
import { Banner, Button, Checkbox, ListSkeleton, showToast } from '../../ui'
import { getBatch, launchBatch, usageKeys, type BatchTask } from './usageApi'
import { batchOutcome, launchLabel, newAttemptKey } from './usageModel'

/**
 * Use it before it resets (U4): the batch of ready tasks this account could
 * run before its window resets, ranked and sized on the server. Ticked tasks
 * launch together, each in its own project's environment on this account.
 * A capacity alert's Review opens this with the batch already loaded.
 */
export function UsageBatch({
  accountId,
  initiallyOpen,
  windowKey,
}: {
  accountId: string
  initiallyOpen: boolean
  windowKey: string | null
}) {
  const [open, setOpen] = useState(initiallyOpen)
  const batch = useQuery({ queryKey: usageKeys.batch(accountId, windowKey), queryFn: () => getBatch(accountId, windowKey), enabled: open })
  const heading = useRef<HTMLHeadingElement>(null)
  useEffect(() => {
    if (initiallyOpen) heading.current?.focus({ preventScroll: false })
  }, [initiallyOpen])
  return (
    <div className="aw-usage__block" id={initiallyOpen ? 'batch' : undefined}>
      <div className="aw-usage__blockhead">
        <h3 className="aw-usage__blocktitle" ref={heading} tabIndex={-1}>
          Use it before it resets
        </h3>
        <Button aria-expanded={open} onClick={() => setOpen((value) => !value)} size="sm" variant="ghost">
          {open ? 'Hide' : 'Show the batch'}
        </Button>
      </div>
      {!open ? (
        <p className="aw-usage__at">Ready tasks across your projects that this account could run before its window resets.</p>
      ) : batch.isPending ? (
        <ListSkeleton label="Loading the batch" rows={3} />
      ) : batch.isError ? (
        <Banner tone="danger">
          The batch didn’t load. <Button onClick={() => void batch.refetch()} size="sm" variant="ghost">Try again</Button>
        </Banner>
      ) : (
        <BatchEditor
          accountId={accountId}
          key={batch.dataUpdatedAt}
          more={batch.data.more}
          skipped={batch.data.skipped}
          summary={batch.data.capacity.summary}
          tasks={batch.data.tasks}
          windowKey={windowKey}
        />
      )}
    </div>
  )
}

function BatchEditor({
  accountId,
  more,
  skipped,
  summary,
  tasks,
  windowKey,
}: {
  accountId: string
  more: BatchTask[]
  skipped: { projectId: string; projectName: string; reason: string; href: string }[]
  summary: string
  tasks: BatchTask[]
  windowKey: string | null
}) {
  const queryClient = useQueryClient()
  const [picked, setPicked] = useState(() => new Set(tasks.map((task) => task.id)))
  // One key per batch as shown, so a repeated click launches it once.
  const [attempt, setAttempt] = useState(() => newAttemptKey('batch'))
  const all = [...tasks, ...more]
  const chosen = all.filter((task) => picked.has(task.id))
  const launch = useMutation({
    mutationFn: () =>
      launchBatch(
        accountId,
        { tasks: chosen.map((task) => ({ projectId: task.projectId, workItemId: task.id })), windowKey },
        attempt,
      ),
    onSuccess: (result) => {
      showToast(batchOutcome(result))
      setAttempt(newAttemptKey('batch'))
      void queryClient.invalidateQueries({ queryKey: usageKeys.root })
    },
  })
  const toggle = (id: string, on: boolean) => {
    setPicked((current) => {
      const next = new Set(current)
      if (on) next.add(id)
      else next.delete(id)
      return next
    })
  }
  return (
    <div className="aw-batch">
      <p className="aw-usage__at">{summary}</p>
      {launch.isError ? <Banner tone="danger">{describeError(launch.error).message}</Banner> : null}
      {all.length === 0 ? (
        <p className="aw-usage__at">
          Nothing is ready for an agent in the projects this account runs for. A task is ready when its blockers are done and it has acceptance
          criteria, a description and a repository.
        </p>
      ) : (
        <ul aria-label="Tasks in the batch" className="aw-batch__tasks">
          {all.map((task, index) => (
            <li className={index === tasks.length && index > 0 ? 'aw-batch__task is-more' : 'aw-batch__task'} key={task.id}>
              <Checkbox
                checked={picked.has(task.id)}
                description={[task.projectName, task.environmentName, ...task.reasons].filter(Boolean).join(' · ')}
                label={
                  <>
                    <span className="aw-usage__key">{task.key}</span> {task.title}
                  </>
                }
                onChange={(on) => toggle(task.id, on)}
              />
              <Link aria-label={`Open ${task.key}`} className="aw-btn aw-btn--ghost aw-btn--sm" to={task.href}>
                <span className="aw-btn__label">Open</span>
              </Link>
            </li>
          ))}
        </ul>
      )}
      {skipped.length > 0 ? (
        <details className="aw-batch__skipped">
          <summary>
            {skipped.length === 1 ? '1 project can’t join' : `${skipped.length} projects can’t join`}
          </summary>
          <ul>
            {skipped.map((project) => (
              <li key={project.projectId}>
                <Link to={project.href}>{project.projectName}</Link>: {project.reason}
              </li>
            ))}
          </ul>
        </details>
      ) : null}
      {all.length > 0 ? (
        <div className="aw-batch__actions">
          <Button
            busy={launch.isPending}
            disabled={chosen.length === 0}
            icon="play"
            onClick={() => launch.mutate()}
            variant="primary"
          >
            {launchLabel(chosen.length)}
          </Button>
        </div>
      ) : null}
    </div>
  )
}
