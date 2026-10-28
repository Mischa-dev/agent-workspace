import { lazy, Suspense } from 'react'

import { useReadiness } from '../ai/hooks'
import { useProjectExperience } from '../experience/layout'
import { Button, Icon } from '../ui'
import type { OpenTask } from './TaskConnections'
import { TaskGlyph } from './TaskGlyph'
import type { WorkItem } from './types'

// Launching loads with the launch pages' code; the space it takes is held meanwhile.
const LaunchCard = lazy(() => import('../launch/LaunchCard').then((module) => ({ default: module.LaunchCard })))

export type StartFix = 'criterion' | 'description' | 'link'

// What an agent still needs, as a fix the person can make from here.
const FIXES: Record<string, { fix: StartFix; label: string }> = {
  criteria: { fix: 'criterion', label: 'Add acceptance criteria' },
  description: { fix: 'description', label: 'Describe it in more detail' },
  repository: { fix: 'link', label: 'Link a repository' },
}

/**
 * Whether the task can start, said once, next to the button that starts an
 * agent on it: ready, waiting on other tasks, or marked blocked (the reason
 * is in the note under the title). When only an agent is held back from
 * picking it up, it says what's missing, with a way to add it.
 */
export function TaskStart({
  canFix = () => true,
  disabled,
  item,
  onFix,
  onOpenTask,
}: {
  /** Fixes for a section the layout hides are named but not offered. */
  canFix?: (fix: StartFix) => boolean
  disabled: boolean
  item: WorkItem
  onFix: (fix: StartFix) => void
  onOpenTask: OpenTask
}) {
  const readiness = useReadiness(item.projectId, item.id, item.version)
  const keys = useProjectExperience(item.projectId).shows('tech.task_keys')
  const category = item.status.semanticCategory
  const finished = category === 'completed' || category === 'canceled'
  const notStarted = category === 'backlog' || category === 'unstarted'
  const waiting = item.blockedBy.filter((task) => !task.finished)
  // Blockers in projects this person can't see only show in the server's own words.
  const waitingUnseen =
    !finished && !waiting.length && readiness.data && !readiness.data.ready
      ? readiness.data.checks.find((check) => check.id === 'dependencies' && !check.ok)?.label
      : undefined
  // Agents pick up tasks that haven't started yet.
  const missing =
    notStarted && !item.claim && readiness.data?.ready && !readiness.data.readyForAgents
      ? readiness.data.checks.filter((check) => check.forAgents && !check.ok && FIXES[check.id])
      : []
  const showState = !finished && (waiting.length > 0 || Boolean(waitingUnseen) || Boolean(item.blocked) || notStarted)
  if (disabled && !showState && !missing.length) return null

  return (
    <section aria-label="Starting work" className="aw-taskstart">
      {!showState ? null : waiting.length ? (
        <p className="aw-taskstart__state is-waiting">
          <Icon name="clock" size={16} />
          <span>
            Waiting on{' '}
            {waiting.slice(0, 3).map((task, index) => (
              <span key={task.id}>
                {index ? ', ' : ''}
                <a
                  className="aw-taskstart__key"
                  href={task.href}
                  onClick={(event) => {
                    if (event.metaKey || event.ctrlKey || event.shiftKey) return
                    event.preventDefault()
                    onOpenTask(task)
                  }}
                  title={task.title}
                >
                  {keys ? task.key : task.title}
                </a>
              </span>
            ))}
            {waiting.length > 3 ? ` and ${waiting.length - 3} more` : ''}
          </span>
        </p>
      ) : waitingUnseen ? (
        <p className="aw-taskstart__state is-waiting">
          <Icon name="clock" size={16} />
          {waitingUnseen}
        </p>
      ) : item.blocked ? (
        <p className="aw-taskstart__state is-blocked">
          <TaskGlyph name="blocked" size={16} />
          Marked blocked
        </p>
      ) : (
        <p className="aw-taskstart__state is-ready">
          <Icon name="circle-check" size={16} />
          Ready to start
        </p>
      )}
      {missing.length ? (
        <div className="aw-taskstart__missing">
          <p className="aw-taskstart__hint">Before an agent can pick it up:</p>
          <ul className="aw-taskstart__fixes">
            {missing.map((check) => (
              <li key={check.id}>
                {disabled || !canFix(FIXES[check.id].fix) ? (
                  <span className="aw-taskstart__fix">{FIXES[check.id].label}</span>
                ) : (
                  <Button className="aw-taskstart__fix" icon="plus" onClick={() => onFix(FIXES[check.id].fix)} size="sm" variant="ghost">
                    {FIXES[check.id].label}
                  </Button>
                )}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {disabled ? null : (
        <Suspense fallback={<div aria-hidden="true" className="aw-taskstart__launching" />}>
          <LaunchCard embedded item={item} />
        </Suspense>
      )}
    </section>
  )
}
