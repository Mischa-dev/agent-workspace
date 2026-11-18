import { useRef, useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'

import { queryKeys } from '../api/queryKeys'
import { createChannel } from '../conversations/sidebarApi'
import { Button, Checkbox, Dialog, TextField } from '../ui'
import { spaceHref } from '../app/href'

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
  const [isPrivate, setPrivate] = useState(false)
  const [everyone, setEveryone] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const value = name.trim().replace(/^#/, '')

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!value || busy) return
    setBusy(true)
    setError(null)
    void createChannel({ projectId, name: value, private: isPrivate, autoJoin: everyone, idempotencyKey: idempotencyKey.current })
      .then((channel) => {
        idempotencyKey.current = crypto.randomUUID()
        setName('')
        setPrivate(false)
        setEveryone(true)
        onClose()
        void queryClient.invalidateQueries({ queryKey: queryKeys.conversationSidebarRoot(projectId) })
        onCreated(spaceHref(projectId, `/channels/${encodeURIComponent(channel.id)}`))
      })
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : 'The channel wasn’t created. Try again.'))
      .finally(() => setBusy(false))
  }

  return (
    <Dialog
      description="Channels are where a project’s conversations happen. Everyone in the project can read and post, unless you make it private."
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
          onChange={(event) => {
            setName(event.target.value)
            idempotencyKey.current = crypto.randomUUID()
          }}
          prefix="#"
          value={name}
        />
        <Checkbox
          checked={isPrivate}
          description="Only people you add can find it and read it. You can add them once it’s made."
          label="Private"
          onChange={(checked) => {
            setPrivate(checked)
            idempotencyKey.current = crypto.randomUUID()
          }}
        />
        {isPrivate ? null : (
          <Checkbox
            checked={everyone}
            description="They’ll find it in their sidebar, and so will everyone who joins the space later. Otherwise people join it from Browse channels."
            label="Everyone joins it"
            onChange={(checked) => {
              setEveryone(checked)
              idempotencyKey.current = crypto.randomUUID()
            }}
          />
        )}
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
