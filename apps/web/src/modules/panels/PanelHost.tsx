// A connected module's panel (M1.14): asks the module for it through
// `module_panel_open`, draws its blocks, and sends a press or a form
// through `module_panel_press`. The module answers with the next panel,
// errors for its form, `done` (the flow is finished; whoever opened the
// panel gets its result) or `close`. The panel's `state` goes back with
// each request as the module gave it, and `refresh.afterSeconds` asks again.
// The server checks every answer before it gets here (blocks.rs).

import { useQuery } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'

import { callOp, newRequestId, opKey, type OpOutput } from '../../api/ops'
import { Banner, Button, ErrorState, ListSkeleton } from '../../ui'
import { describeError } from '../../ui/errorText'
import { PanelBlocks, type PanelAction, type PanelBlock } from './StopgapBlocks'

type Answer = OpOutput<'module_panel_open'>['answer']

export type PanelPlace = {
  moduleId: string
  panel: string
  spaceId?: string
  thing?: { kind: string; id: string }
  workspaceId?: string
}

type Drawn = { title?: string; blocks: PanelBlock[]; state?: string; refreshAfter?: number }

function drawn(panel: Record<string, unknown>): Drawn {
  const refresh = panel.refresh as { afterSeconds?: unknown } | undefined
  return {
    title: typeof panel.title === 'string' ? panel.title : undefined,
    blocks: Array.isArray(panel.blocks) ? (panel.blocks as PanelBlock[]) : [],
    state: typeof panel.state === 'string' ? panel.state : undefined,
    refreshAfter: typeof refresh?.afterSeconds === 'number' ? refresh.afterSeconds : undefined,
  }
}

export function PanelHost({
  label,
  onClose,
  onDone,
  place,
}: {
  /** The panel's name, for people using a screen reader: its manifest title. */
  label: string
  onClose?: () => void
  /** The flow finished; `result` is what the module handed back. */
  onDone?: (done: { message?: string | null; result?: Record<string, unknown> | null }) => void
  place: PanelPlace
}) {
  const { moduleId, panel, spaceId, thing, workspaceId } = place
  const input = { moduleId, panel, spaceId, thing }
  const opened = useQuery({
    queryKey: opKey('module_panel_open', input, workspaceId),
    queryFn: ({ signal }) => callOp('module_panel_open', input, { workspaceId, signal, retries: 0 }),
    // An app's panel is asked for when it's shown, not kept.
    gcTime: 0,
    staleTime: Infinity,
    retry: false,
    // Each answer is new, so it replaces what a press drew before it.
    structuralSharing: false,
  })
  // What a press answered replaces what opening drew, until it opens again.
  type After = { from: unknown; drawn?: Drawn; errors?: Record<string, string>; finished?: string }
  const [after, setAfter] = useState<After | null>(null)
  const [failed, setFailed] = useState<unknown>(null)
  const pressing = useRef(false)

  const answered = opened.data?.answer
  const mine = after && after.from === opened.data ? after : null
  const shown = mine?.drawn ?? (answered?.kind === 'panel' ? drawn(answered.panel) : null)
  const errors = mine?.errors ?? {}
  const finished = mine?.finished ?? (answered?.kind === 'done' ? (answered.message ?? 'Done.') : null)
  const state = shown?.state

  // Opening can finish or close the flow at once.
  useEffect(() => {
    if (answered?.kind === 'done') onDone?.({ message: answered.message, result: answered.result })
    if (answered?.kind === 'close') onClose?.()
  }, [answered, onClose, onDone])

  const take = (answer: Answer) => {
    const from = opened.data
    setFailed(null)
    switch (answer.kind) {
      case 'panel':
        setAfter({ from, drawn: drawn(answer.panel) })
        break
      case 'errors': {
        const messages = Object.fromEntries(Object.entries(answer.errors).map(([field, message]) => [field, String(message)]))
        setAfter((last) => ({ ...(last?.from === from ? last : {}), from, errors: messages }))
        break
      }
      case 'done':
        setAfter((last) => ({ ...(last?.from === from ? last : {}), from, finished: answer.message ?? 'Done.' }))
        onDone?.({ message: answer.message, result: answer.result })
        break
      case 'close':
        onClose?.()
        break
    }
  }

  // The module asked to be asked again after a while.
  const { refetch, dataUpdatedAt } = opened
  const refreshAfter = shown?.refreshAfter
  useEffect(() => {
    const seconds = refreshAfter
    if (!seconds || finished) return
    const timer = window.setTimeout(() => {
      if (!pressing.current) void refetch()
    }, seconds * 1000)
    return () => window.clearTimeout(timer)
  }, [refreshAfter, finished, refetch, dataUpdatedAt, after])

  const onAction = async (action: PanelAction) => {
    pressing.current = true
    try {
      const value = await callOp(
        'module_panel_press',
        { ...input, state, action: { id: action.id, values: action.values } },
        { workspaceId, requestId: newRequestId() },
      )
      take(value.answer)
    } catch (error) {
      setFailed(error)
      throw new Error(describeError(error).message, { cause: error })
    } finally {
      pressing.current = false
    }
  }

  if (opened.isPending) return <ListSkeleton />
  if (opened.isError) {
    return (
      <ErrorState error={opened.error} onRetry={() => void opened.refetch()} title="The app didn’t answer">
        {describeError(opened.error).message}
      </ErrorState>
    )
  }
  if (finished) {
    return (
      <div className="aw-panel-host" data-part="panel-host">
        <Banner tone="success">{finished}</Banner>
        <Button
          onClick={() => {
            setAfter(null)
            void opened.refetch()
          }}
          variant="secondary"
        >
          Back to {label}
        </Button>
      </div>
    )
  }
  return (
    <div className="aw-panel-host" data-part="panel-host">
      {failed ? <Banner tone="danger">{describeError(failed).message}</Banner> : null}
      {shown ? <PanelBlocks blocks={shown.blocks} errors={errors} label={shown.title ?? label} onAction={onAction} /> : null}
    </div>
  )
}
