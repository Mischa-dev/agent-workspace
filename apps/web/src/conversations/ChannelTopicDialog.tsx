import { useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { callOp } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
import { Button, Checkbox, Dialog, TextField, describeError } from '../ui'

/** Set a channel's topic, and for its managers whether everyone joins it (C2.7). */
export function ChannelTopicDialog({
  autoJoin,
  canManage,
  channelId,
  isPrivate,
  onClose,
  open,
  projectId,
  topic,
  version,
}: {
  autoJoin: boolean
  canManage: boolean
  channelId: string
  isPrivate: boolean
  onClose: () => void
  open: boolean
  projectId: string
  topic: string
  version: number
}) {
  const queryClient = useQueryClient()
  const [value, setValue] = useState(topic)
  const [everyone, setEveryone] = useState(autoJoin)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const changedTopic = value.trim() !== topic
  const changedJoin = everyone !== autoJoin

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (busy || (!changedTopic && !changedJoin)) return onClose()
    setBusy(true)
    setError(null)
    void callOp('channel_update', {
      channelId,
      expectedVersion: version,
      ...(changedTopic ? { topic: value.trim() } : {}),
      ...(changedJoin ? { autoJoin: everyone } : {}),
    })
      .then(async () => {
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: queryKeys.channelDetails(projectId, channelId) }),
          queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) }),
        ])
        onClose()
      })
      .catch((reason: unknown) => setError(describeError(reason).message))
      .finally(() => setBusy(false))
  }

  return (
    <Dialog
      dirty={changedTopic}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} type="submit" variant="primary">
            Save
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open={open}
      title="Channel topic"
    >
      <TextField
        autoComplete="off"
        error={error ?? undefined}
        hint="One line on what the channel is on right now. Leave it empty to clear it."
        label="Topic"
        maxLength={250}
        onChange={(event) => setValue(event.target.value)}
        value={value}
      />
      {canManage && !isPrivate ? (
        <Checkbox
          checked={everyone}
          description="Turning this on adds everyone in the space now, and everyone who joins it later."
          label="Everyone joins it"
          onChange={setEveryone}
        />
      ) : null}
    </Dialog>
  )
}
