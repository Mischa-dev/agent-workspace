import { useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { callOp } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
import { Button, Checkbox, Dialog, TextField, describeError } from '../ui'

/**
 * Set a channel's topic, whether what's said there is working notes by
 * default (AWD-220), and for its managers whether everyone joins it (C2.7).
 */
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
  workingNotes,
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
  workingNotes: boolean
}) {
  const queryClient = useQueryClient()
  const [value, setValue] = useState(topic)
  const [everyone, setEveryone] = useState(autoJoin)
  const [notes, setNotes] = useState(workingNotes)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const changedTopic = value.trim() !== topic
  const changedJoin = everyone !== autoJoin
  const changedNotes = notes !== workingNotes

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (busy || (!changedTopic && !changedJoin && !changedNotes)) return onClose()
    setBusy(true)
    setError(null)
    void callOp('channel_update', {
      channelId,
      expectedVersion: version,
      ...(changedTopic ? { topic: value.trim() } : {}),
      ...(changedJoin ? { autoJoin: everyone } : {}),
      ...(changedNotes ? { workingNotes: notes } : {}),
    })
      .then(async () => {
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: queryKeys.channelDetails(projectId, channelId) }),
          queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) }),
          ...(changedNotes ? [queryClient.invalidateQueries({ queryKey: queryKeys.timeline(projectId, channelId, false) })] : []),
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
      title="Channel settings"
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
      <Checkbox
        checked={notes}
        description="What’s said here is kept and anyone in the channel can read it, but it doesn’t mark the channel unread or notify anyone it doesn’t mention. Good for agents working things out between themselves."
        label="Working channel"
        onChange={setNotes}
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
