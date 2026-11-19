import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp } from '../api/ops'
import { toastError } from '../app/mutations'
import { Button, Dialog, ErrorState, TextArea, formatRelative } from '../ui'

/**
 * A channel's or thread's memory page (A2.16): what anyone working there,
 * people and agents alike, should know first. Agents get its first lines
 * when they open the conversation or take a request in it. Every version
 * is kept, like any page.
 */
export function MemoryDialog({
  conversationId,
  onClose,
  open,
}: {
  conversationId: string
  onClose: () => void
  open: boolean
}) {
  const queryClient = useQueryClient()
  const key = ['memory-page', conversationId]
  const memory = useQuery({
    queryKey: key,
    queryFn: () => callOp('memory_page_open', { conversation: conversationId }, { retries: 0 }),
    enabled: open,
  })
  const page = memory.data?.page ?? null
  // What the person typed; until then, the page as it is.
  const [draft, setDraft] = useState<string | null>(null)
  const body = draft ?? page?.body ?? ''
  const [saving, setSaving] = useState(false)

  const save = () => {
    setSaving(true)
    void callOp('memory_page_set', { conversation: conversationId, body, expectedVersion: page?.version ?? 0 })
      .then(() => queryClient.invalidateQueries({ queryKey: key }))
      .then(onClose)
      .catch((reason: unknown) => toastError('Memory wasn’t saved', reason))
      .finally(() => setSaving(false))
  }

  const changed = body !== (page?.body ?? '')
  return (
    <Dialog
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={saving} disabled={!memory.isSuccess || !changed} onClick={save} variant="primary">
            Save
          </Button>
        </>
      }
      onClose={onClose}
      open={open}
      title="Memory"
    >
      {memory.isError ? <ErrorState error={memory.error} onRetry={() => void memory.refetch()} title="This didn’t load" /> : null}
      {memory.isSuccess ? (
        <TextArea
          hint={
            page
              ? `Edited ${formatRelative(page.updatedAt)}. Agents read the start of it before they act here.`
              : 'Durable facts for anyone working here. Agents read the start of it before they act here.'
          }
          label="What everyone here should know"
          maxLength={200000}
          onChange={(event) => setDraft(event.target.value)}
          rows={12}
          value={body}
        />
      ) : null}
    </Dialog>
  )
}
