import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'

import { callOp, type OpInput } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
import { Button, Dialog, Kbd, TextArea, describeError, showToast } from '../ui'
import { isTypingTarget } from './shortcutKeys'
import { TaskSearchPicker } from './TaskSearchPicker'
import { snoozeChoices } from './triageModel'

type Decision = Omit<OpInput<'triage_decide'>, 'taskIds'>

/**
 * The triage queue's decisions (T1.23) for the task open beside it: 1
 * accepts, 2 declines with a reason, 3 marks it a duplicate and H snoozes
 * it. After each, the panel closes, and ↓ or J opens the next.
 */
export function TriageBar({
  disabled,
  onDone,
  projectId,
  taskId,
}: {
  disabled: boolean
  /** The decision landed: move on to the next task. */
  onDone: () => void
  projectId: string
  /** The task open in the side panel, if any. */
  taskId: string | null
}) {
  const client = useQueryClient()
  const [busy, setBusy] = useState(false)
  const [asking, setAsking] = useState<'decline' | 'snooze' | null>(null)
  const [reason, setReason] = useState('')
  const duplicateButton = useRef<HTMLButtonElement>(null)
  // The duplicate picker, open beside the button it was opened from.
  const [picking, setPicking] = useState<HTMLElement | null>(null)
  const ready = Boolean(taskId) && !disabled && !busy

  const decide = async (decision: Decision, done: string) => {
    if (!taskId) return
    setBusy(true)
    try {
      await callOp('triage_decide', { taskIds: [taskId], ...decision }, { requestId: crypto.randomUUID() })
      showToast({ tone: 'success', title: done })
      setAsking(null)
      setReason('')
      onDone()
      void client.invalidateQueries({ queryKey: queryKeys.workItemsRoot(projectId) })
    } catch (error) {
      showToast({ tone: 'danger', title: describeError(error).message })
    } finally {
      setBusy(false)
    }
  }
  const decideRef = useRef(decide)
  useEffect(() => {
    decideRef.current = decide
  })

  useEffect(() => {
    if (!ready) return undefined
    const onKey = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey || isTypingTarget(event.target)) return
      const key = event.key.toLowerCase()
      if (key === '1') void decideRef.current({ action: 'accept' }, 'Accepted')
      else if (key === '2') setAsking('decline')
      else if (key === '3') setPicking(duplicateButton.current)
      else if (key === 'h') setAsking('snooze')
      else return
      event.preventDefault()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [ready])

  return (
    <div aria-label="Triage" className="aw-triagebar" role="toolbar">
      <span className="aw-triagebar__hint">{taskId ? 'Decide on the open task' : 'Open a task to decide on it (↓ or J)'}</span>
      <Button disabled={!ready} onClick={() => void decide({ action: 'accept' }, 'Accepted')} size="sm" variant="primary">
        Accept <Kbd>1</Kbd>
      </Button>
      <Button disabled={!ready} onClick={() => setAsking('decline')} size="sm">
        Decline <Kbd>2</Kbd>
      </Button>
      <Button disabled={!ready} onClick={(event) => setPicking(event.currentTarget)} ref={duplicateButton} size="sm">
        Duplicate <Kbd>3</Kbd>
      </Button>
      <Button disabled={!ready} onClick={() => setAsking('snooze')} size="sm">
        Snooze <Kbd>H</Kbd>
      </Button>
      {picking && taskId ? (
        <TaskSearchPicker
          anchor={picking}
          exclude={[taskId]}
          label="The task this one duplicates"
          onClose={() => setPicking(null)}
          onPick={(target) => {
            setPicking(null)
            if (target.id) void decide({ action: 'duplicate', duplicateOf: target.id }, 'Marked a duplicate')
            else if (target.key) void decide({ action: 'duplicate', duplicateOf: target.key }, 'Marked a duplicate')
          }}
          projectId={projectId}
        />
      ) : null}
      <Dialog
        description="It moves to the workflow's canceled status. A reason is posted as your comment."
        dirty={reason.trim().length > 0}
        footer={
          <>
            <Button onClick={() => setAsking(null)} variant="ghost">
              Cancel
            </Button>
            <Button busy={busy} type="submit" variant="danger">
              Decline
            </Button>
          </>
        }
        onClose={() => setAsking(null)}
        onSubmit={(event) => {
          event.preventDefault()
          void decide({ action: 'decline', reason: reason.trim() || undefined }, 'Declined')
        }}
        open={asking === 'decline'}
        title="Decline this task?"
      >
        <TextArea autoFocus label="Reason (optional)" maxLength={2000} onChange={(event) => setReason(event.target.value)} rows={3} value={reason} />
      </Dialog>
      <Dialog description="It leaves the queue and comes back then." onClose={() => setAsking(null)} open={asking === 'snooze'} title="Snooze until">
        <div className="aw-triagebar__snooze">
          {snoozeChoices(new Date()).map((choice, index) => (
            <Button autoFocus={index === 0} disabled={busy} key={choice.label} onClick={() => void decide({ action: 'snooze', until: choice.until.toISOString() }, `Snoozed until ${choice.label.toLowerCase()}`)}>
              {choice.label}
            </Button>
          ))}
        </div>
      </Dialog>
    </div>
  )
}
