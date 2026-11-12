import { useState, type FormEvent } from 'react'

import { requestJson } from '../../api/client'
import { isApiError } from '../../api/errors'
import { Button, TextField } from '../../ui'
import { embedPath, moduleEmbedTarget } from './moduleEmbed'

/**
 * Embeds something of the app in a page (AWD-53), from the page editor's
 * toolbar: a pasted address of a task, page, whiteboard, thread or task list
 * becomes the piece its module declares. `onCollection` offers a collection
 * view instead, where collections are on, and `onSheet` a sheet's range.
 */
export default function EmbedPicker({
  onClose,
  onCollection,
  onPick,
  onSheet,
}: {
  onClose: () => void
  onCollection: (() => void) | null
  onPick: (embed: { href: string; label: string }) => void
  onSheet: (() => void) | null
}) {
  const [address, setAddress] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = (event: FormEvent) => {
    event.preventDefault()
    const href = embedPath(address)
    const target = href ? moduleEmbedTarget(href) : null
    if (!href || !target) {
      setError('Paste the address of a task, page, whiteboard, thread or task list here.')
      return
    }
    if (target.kind === 'tasks') {
      onPick({ href, label: target.decl.many })
      return
    }
    // The embed keeps the thing's name, so the page reads well as Markdown.
    setBusy(true)
    void requestJson<{ thing: { title: string } }>(`/api/v1/things/${encodeURIComponent(target.thing)}/${encodeURIComponent(target.id)}`)
      .then((body) => onPick({ href, label: body.thing.title }))
      .catch((failure: unknown) =>
        setError(isApiError(failure) && failure.status === 404 ? `That ${target.decl.one.toLowerCase()} isn’t here, or you can’t see it.` : 'That didn’t load. Try again.'),
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
        {onCollection ? (
          <Button onClick={onCollection} size="sm">
            Collection view…
          </Button>
        ) : null}
        {onSheet ? (
          <Button onClick={onSheet} size="sm">
            Sheet…
          </Button>
        ) : null}
        <Button onClick={onClose} size="sm" variant="ghost">
          Cancel
        </Button>
      </span>
    </form>
  )
}
