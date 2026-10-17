import { useState } from 'react'

import { readLocal, writeLocal } from '../app/localDrafts'

/** A new task being written, kept in this browser per project until it's created. */
export type TaskDraft = {
  title: string
  description: string
  /** Kept with the draft so sending it again after a reload can't make two tasks. */
  idempotencyKey: string
  /** What it started from, when that was something shared. */
  source?: string
}

function blank(): TaskDraft {
  return { title: '', description: '', idempotencyKey: crypto.randomUUID() }
}

const storageKey = (projectId: string) => `task-draft:${projectId}`

export function readTaskDraft(projectId: string): TaskDraft | null {
  const stored = projectId ? readLocal<TaskDraft>(storageKey(projectId)) : null
  return stored && typeof stored.title === 'string' && typeof stored.idempotencyKey === 'string'
    ? {
        title: stored.title,
        description: typeof stored.description === 'string' ? stored.description : '',
        idempotencyKey: stored.idempotencyKey,
        source: typeof stored.source === 'string' ? stored.source : undefined,
      }
    : null
}

/**
 * The draft for a new task in `projectId`, saved as it's typed and restored
 * after a reload. `start` (something just shared) replaces what was saved.
 */
export function useTaskDraft(projectId: string, start?: { title: string; description: string }) {
  const initial = () => {
    const saved = readTaskDraft(projectId)
    if (!start) return saved ?? blank()
    // A reload of the same share keeps its edits; a new share starts over.
    const source = JSON.stringify(start)
    return saved?.source === source ? saved : { ...blank(), ...start, source }
  }
  const [state, setState] = useState(() => ({ projectId, draft: initial() }))
  // Another project has its own draft.
  const current = state.projectId === projectId ? state.draft : initial()
  if (state.projectId !== projectId) setState({ projectId, draft: current })

  const update = (patch: Partial<Pick<TaskDraft, 'title' | 'description'>>) => {
    setState((previous) => {
      const draft = { ...(previous.projectId === projectId ? previous.draft : current), ...patch }
      if (projectId) writeLocal(storageKey(projectId), draft.title.trim() || draft.description.trim() ? draft : null)
      return { projectId, draft }
    })
  }
  const clear = () => {
    if (projectId) writeLocal(storageKey(projectId), null)
    setState({ projectId, draft: blank() })
  }
  return { draft: current, update, clear }
}
