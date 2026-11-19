import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp } from '../api/ops'
import { toastError } from '../app/mutations'
import { Button, Dialog, EmptyState, ErrorState, Switch } from '../ui'

/**
 * Who takes part here on their own (A2.9): each account that takes reply
 * requests and can see the channel or thread, with ambient mode on or off.
 * In ambient mode it hears every message and is invited to reply to each
 * one nobody prompted, waiting a few minutes after it answers.
 */
export function AmbientAgentsDialog({
  conversationId,
  onClose,
  open,
}: {
  conversationId: string
  onClose: () => void
  open: boolean
}) {
  const queryClient = useQueryClient()
  const key = ['conversation-agent-settings', conversationId]
  const listed = useQuery({
    queryKey: key,
    queryFn: () => callOp('conversation_agent_settings_list', { conversation: conversationId }, { retries: 0 }),
    enabled: open,
  })
  const [busy, setBusy] = useState<string | null>(null)

  const set = (accountId: string, displayName: string, ambient: boolean) => {
    setBusy(accountId)
    void callOp('conversation_agent_settings_set', { conversation: conversationId, account: accountId, ambient })
      .then(() => queryClient.invalidateQueries({ queryKey: key }))
      .catch((reason: unknown) => toastError(`${displayName} wasn’t changed`, reason))
      .finally(() => setBusy(null))
  }

  const accounts = listed.data?.accounts ?? []
  return (
    <Dialog footer={<Button onClick={onClose}>Done</Button>} onClose={onClose} open={open} title="Ambient mode">
      {listed.isError ? <ErrorState error={listed.error} onRetry={() => void listed.refetch()} title="This didn’t load" /> : null}
      {listed.isSuccess && accounts.length === 0 ? (
        <EmptyState title="Nobody here takes reply requests">Add an agent to this space to have it join in here.</EmptyState>
      ) : null}
      {accounts.map((account) => (
        <Switch
          checked={account.ambient}
          description={
            account.ambient
              ? `Hears every message and may reply to any. Waits ${Math.round(account.replyCooldownSeconds / 60)} min after answering.`
              : 'Replies only when mentioned.'
          }
          disabled={busy !== null}
          key={account.accountId}
          label={account.displayName}
          onChange={(ambient) => set(account.accountId, account.displayName, ambient)}
        />
      ))}
    </Dialog>
  )
}
