import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { callOp, invalidateOps } from '../api/ops'
import type { ChangeSetView } from '../api/generated/operations'
import { Button, describeError, formatFullDate, formatRelative } from '../ui'
import { describeTouched, type ChangeKind } from './recentEdits'
import './recentEdits.css'

/** The "Recent edits" toggle an editor's toolbar shows. */
export function RecentEditsToggle({ on, onChange }: { on: boolean; onChange: (on: boolean) => void }) {
  return (
    <Button aria-pressed={on} onClick={() => onChange(!on)} size="sm" variant={on ? 'secondary' : 'ghost'}>
      Recent edits
    </Button>
  )
}

/** Who made the newest outlined change, when, and an Undo for it. */
export function RecentEditsBar({ canUndo, change, kind, onUndone }: { kind: ChangeKind; change: ChangeSetView | null; canUndo: boolean; onUndone?: () => void }) {
  const queryClient = useQueryClient()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  if (!change) return null
  const undo = async () => {
    setBusy(true)
    setError(null)
    try {
      const undone = await callOp('change_undo', { changeSetId: change.id })
      if (undone.skipped.length > 0) setError(`Left as they are, changed since: ${undone.skipped.slice(0, 5).join(', ')}${undone.skipped.length > 5 ? '…' : ''}`)
      await invalidateOps(queryClient, ['change_list'])
      onUndone?.()
    } catch (problem) {
      setError(`That change wasn’t undone. ${describeError(problem).message}`)
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="aw-recent-edits" role="status">
      <span className="aw-recent-edits__swatch" aria-hidden="true" />
      <span>
        {change.actor ?? 'Someone'} changed {describeTouched(kind, change)} ·{' '}
        <time dateTime={change.updatedAt} title={formatFullDate(change.updatedAt)}>
          {formatRelative(change.updatedAt)}
        </time>
      </span>
      {canUndo && change.undoable ? (
        <Button busy={busy} onClick={() => void undo()} size="sm" variant="ghost">
          Undo
        </Button>
      ) : null}
      {error ? <span className="aw-recent-edits__note">{error}</span> : null}
    </div>
  )
}
