import { useEffect, useRef } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'

import { here } from '../app/href'
import { useAuthenticatedSession } from '../auth/session'
import { onRunFinished } from '../realtime/runFinished'
import { showToast, type ToastTone } from '../ui'
import { viewRun } from './api'
import { runTitle } from './runModel'

const ENDINGS: Partial<Record<string, { tone: ToastTone; said: string }>> = {
  done: { tone: 'success', said: 'finished' },
  failed: { tone: 'danger', said: 'failed' },
  cancelled: { tone: 'info', said: 'was cancelled' },
  interrupted: { tone: 'danger', said: 'was interrupted' },
}

/**
 * A run you asked for that finishes while you're on another page says so
 * (W1.24, roadmap N5): a toast with Open and, when it made one, its pull
 * request. Rendered once inside the session.
 */
export default function RunFinishedToasts() {
  const session = useAuthenticatedSession()
  const navigate = useNavigate()
  const location = useLocation()
  const pathname = useRef(location.pathname)
  useEffect(() => {
    pathname.current = location.pathname
  }, [location.pathname])
  const actorId = session.user?.actorId ?? null

  useEffect(() => {
    if (!actorId) return undefined
    let live = true
    const stop = onRunFinished((runId) => {
      const runPage = here(`/runs/${encodeURIComponent(runId)}`)
      if (pathname.current.endsWith(`/runs/${encodeURIComponent(runId)}`)) return
      void viewRun(runId)
        .then((run) => {
          const ending = ENDINGS[run.state]
          if (!live || !ending || run.requestedByActorId !== actorId) return
          showToast({
            id: `run-finished:${run.id}`,
            tone: ending.tone,
            title: `${runTitle(run)} ${ending.said}`,
            description: run.agentName,
            action: { label: 'Open', onSelect: () => navigate(runPage) },
            link: run.pullRequestUrl
              ? { label: run.pullRequestNumber ? `Pull request #${run.pullRequestNumber}` : 'Pull request', href: run.pullRequestUrl }
              : undefined,
          })
        })
        .catch(() => undefined)
    })
    return () => {
      live = false
      stop()
    }
  }, [actorId, navigate])

  return null
}
