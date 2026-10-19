import { useRef, useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { createChannel } from '../conversations/api'
import { Button, Dialog, TextField } from '../ui'

/** The New channel dialog, loaded when someone opens it. */
export default function ChannelCreator({
  onClose,
  onCreated,
  open,
  projectId,
}: {
  onClose: () => void
  onCreated: (to: string) => void
  open: boolean
  projectId: string
}) {
  const queryClient = useQueryClient()
  const idempotencyKey = useRef(crypto.randomUUID())
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const value = name.trim().replace(/^#/, '')

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!value || busy) return
    setBusy(true)
    setError(null)
    void createChannel({ projectId, name: value, idempotencyKey: idempotencyKey.current })
      .then((channel) => {
        idempotencyKey.current = crypto.randomUUID()
        setName('')
        onClose()
        void queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) })
        onCreated(`/projects/${encodeURIComponent(projectId)}/channels/${encodeURIComponent(channel.id)}`)
      })
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : 'The channel wasn’t created. Try again.'))
      .finally(() => setBusy(false))
  }

  return (
    <Dialog
      description="Channels are where a project’s conversations happen. Everyone in the project can read and post."
      onClose={onClose}
      open={open}
      title="New channel"
    >
      <form className="aw-form" id="new-channel-form" onSubmit={submit}>
        <TextField
          autoComplete="off"
          error={error ?? undefined}
          hint="Short and lowercase works best, like design or deploys."
          label="Name"
          maxLength={120}
          onChange={(event) => setName(event.target.value)}
          prefix="#"
          value={name}
        />
        <div className="aw-form__actions">
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} disabled={!value} type="submit" variant="primary">
            Create channel
          </Button>
        </div>
      </form>
    </Dialog>
  )
}
