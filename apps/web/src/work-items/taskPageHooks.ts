import { useEffect, useLayoutEffect, useState, type RefObject } from 'react'
import { useQuery } from '@tanstack/react-query'

import { showToast } from '../ui'
import { useSetWorkItemArchived } from './hooks'
import { conflictCurrent, editErrorMessage } from './taskEdits'
import type { WorkItem } from './types'

// The full page puts the fields in a rail beside the text once it is this wide.
const WIDE_AT = 880

/** Archive and restore, with Undo on the toast. Archiving changes nothing anyone else edits, so a newer version doesn't stop it. */
export function useArchive(item: WorkItem, projectArchived: boolean) {
  const mutation = useSetWorkItemArchived()
  const write = async (archived: boolean, version: number) => {
    const input = { projectId: item.projectId, workItemId: item.id, archived }
    try {
      return await mutation.mutateAsync({ ...input, expectedVersion: version, idempotencyKey: crypto.randomUUID() })
    } catch (error) {
      const theirs = conflictCurrent(error)
      if (!theirs) throw error
      return mutation.mutateAsync({ ...input, expectedVersion: theirs.version, idempotencyKey: crypto.randomUUID() })
    }
  }
  const failed = (archived: boolean) => (error: unknown) =>
    showToast({
      tone: 'danger',
      title: archived ? `${item.key} wasn’t archived.` : `${item.key} wasn’t restored.`,
      description: editErrorMessage(error, archived ? 'Archiving' : 'Restoring'),
    })
  return {
    allowed: item.capabilities.archive && !projectArchived,
    busy: mutation.isPending,
    archive: () => {
      write(true, item.version)
        .then((archived) =>
          showToast({
            tone: 'success',
            title: `${archived.key} archived`,
            description: 'It’s hidden from lists until it’s restored.',
            onUndo: () => write(false, archived.version).catch(failed(false)),
          }),
        )
        .catch(failed(true))
    },
    restore: () => {
      write(false, item.version).catch(failed(false))
    },
  }
}

/** How many whiteboards the task has, from the same query as its Whiteboards section; null until it's known. */
export function useBoardCount(taskId: string) {
  const boards = useQuery({
    queryKey: ['whiteboards', 'linked', 'task', taskId],
    // The whiteboards' API stays in their own chunk until a task page asks.
    queryFn: () => import('../whiteboards/api').then((api) => api.linkedWhiteboards('task', taskId)),
  })
  return boards.data ? (boards.data.whiteboards ?? []).length : null
}

/** Whether the full page has room for the rail. It measures the page, since the sidebar takes part of the window. */
export function useWide(root: RefObject<HTMLElement | null>, enabled: boolean) {
  const [wide, setWide] = useState(() => enabled && typeof window !== 'undefined' && window.innerWidth >= WIDE_AT + 280)
  useLayoutEffect(() => {
    const element = root.current
    if (!enabled || !element || typeof ResizeObserver === 'undefined') return undefined
    const observer = new ResizeObserver(([entry]) => setWide(entry.contentRect.width >= WIDE_AT))
    observer.observe(element)
    return () => observer.disconnect()
  }, [enabled, root])
  return enabled && wide
}

/**
 * E opens the description editor, unless the key is meant for something
 * else: a field, a menu or a dialog. In the side panel it only listens while
 * the panel has focus, so it never edits a task other than the one in view.
 */
export function useEditShortcut(enabled: boolean, edit: RefObject<(() => void) | null>, layout: 'panel' | 'page') {
  useEffect(() => {
    if (!enabled) return undefined
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'e' && event.key !== 'E') return
      if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey || event.repeat || event.defaultPrevented) return
      const target = event.target instanceof HTMLElement ? event.target : null
      if (target?.isContentEditable || target?.closest('input, textarea, select, [role="menu"], [role="listbox"]')) return
      if (document.querySelector('.aw-dialog-layer, .aw-popover')) return
      if (layout === 'panel' && target && target !== document.body && !target.closest('#context-region')) return
      event.preventDefault()
      edit.current?.()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [edit, enabled, layout])
}
