import { useState, type FormEvent } from 'react'

import { requestJson } from '../../api/client'
import { isApiError } from '../../api/errors'
import { Button, TextField } from '../../ui'
import { registeredEmbed } from '../../modules/embeds'
import { embedPath } from './embedModel'

/**
 * Embeds something of the app in a page (AWD-53), from the page editor's
 * toolbar: a pasted address of anything a module registers an embed for (a
 * task, page, whiteboard, thread or task list) becomes that module's embed. `others` are the modules' own
 * pickers (M1.5), such as a collection's view or a sheet's range.
 */
export default function EmbedPicker({
  onClose,
  onPick,
  others,
}: {
  onClose: () => void
  onPick: (embed: { href: string; label: string }) => void
  others: { id: string; label: string; onOpen: () => void }[]
}) {
  const [address, setAddress] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = (event: FormEvent) => {
    event.preventDefault()
    const href = embedPath(address)
    const found = href ? registeredEmbed(href) : null
    if (!href || !found) {
      setError('Paste the address of a task, page, whiteboard, thread or task list here.')
      return
    }
    const { kind, id } = found.params
    if (!kind || !id) {
      onPick({ href, label: found.registration.label ?? '' })
      return
    }
    // The embed keeps the thing's name, so the page reads well as Markdown.
    setBusy(true)
    void requestJson<{ thing: { title: string } }>(`/api/v1/things/${encodeURIComponent(kind)}/${encodeURIComponent(id)}`)
      .then((body) => onPick({ href, label: body.thing.title }))
      .catch((failure: unknown) =>
        setError(isApiError(failure) && failure.status === 404 ? `That ${kind} isn’t here, or you can’t see it.` : 'That didn’t load. Try again.'),
      )
      .finally(() => setBusy(false))
  }

  return (
    <form
      aria-label="Embed in the page"
      className="aw-live__link"
      onKeyDown={(event) => {
        if (event.key !== 'Escape') return
        event.preventDefault()
        onClose()
      }}
      onSubmit={submit}
    >
      <TextField
        autoFocus
        className="aw-live__address"
        compact
        error={error ?? undefined}
        label="Address"
        onChange={(event) => {
          setAddress(event.target.value)
          setError(null)
        }}
        placeholder="Link to a task, page, whiteboard or task list"
        value={address}
      />
      <span className="aw-live__actions">
        <Button busy={busy} disabled={!address.trim()} size="sm" type="submit" variant="primary">
          Add to page
        </Button>
        {others.map((other) => (
          <Button key={other.id} onClick={other.onOpen} size="sm">
            {other.label}
          </Button>
        ))}
        <Button onClick={onClose} size="sm" variant="ghost">
          Cancel
        </Button>
      </span>
    </form>
  )
}
