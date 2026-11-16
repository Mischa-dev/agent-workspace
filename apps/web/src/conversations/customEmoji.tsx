// A workspace's own emoji (C2.19): listed for the reaction picker through
// `custom_emoji_list`, added through `custom_emoji_create`. A reaction names
// one as :name:; the server checks it belongs to the message's workspace.
import { useQueryClient } from '@tanstack/react-query'
import { useState, type FormEvent } from 'react'

import { callOp, newRequestId } from '../api/ops'
import { toastError } from '../app/mutations'
import { useHomeWorkspaceId } from '../experience/layout'
import { Button, Dialog, TextField, showToast } from '../ui'
import { customEmojiKeys } from './customEmojiQuery'

const MAX_BYTES = 65_536

function base64Of(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result).replace(/^data:[^,]*,/, ''))
    reader.onerror = () => reject(reader.error ?? new Error('The file couldn’t be read'))
    reader.readAsDataURL(file)
  })
}

/** "Add an emoji": a name and a small image, for everyone in the workspace. */
export function CustomEmojiDialog({ onClose }: { onClose: () => void }) {
  const workspaceId = useHomeWorkspaceId()
  const queryClient = useQueryClient()
  const [name, setName] = useState('')
  const [file, setFile] = useState<File | null>(null)
  const [busy, setBusy] = useState(false)
  const [requestId] = useState(newRequestId)
  const tidy = name.trim().replace(/^:+|:+$/g, '').toLowerCase()
  const nameError = tidy && !/^[a-z0-9_]{2,32}$/.test(tidy) ? 'Use 2 to 32 lowercase letters, digits and underscores.' : undefined
  const fileError = file && file.size > MAX_BYTES ? 'Use an image of at most 64 KB.' : undefined

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!file || !tidy || nameError || fileError || !workspaceId) return
    setBusy(true)
    try {
      await callOp('custom_emoji_create', { name: tidy, imageBase64: await base64Of(file) }, { workspaceId, requestId })
      await queryClient.invalidateQueries({ queryKey: customEmojiKeys.list(workspaceId) })
      showToast({ tone: 'success', title: `:${tidy}: added` })
      onClose()
    } catch (reason) {
      toastError('The emoji wasn’t added', reason)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      description="A PNG, GIF or WebP of at most 64 KB. Everyone in this workspace can react with it."
      dirty={Boolean(name || file)}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} disabled={!file || !tidy || Boolean(nameError || fileError)} type="submit" variant="primary">
            Add emoji
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => void submit(event)}
      open
      title="Add an emoji"
    >
      <TextField autoFocus error={nameError} label="Name" onChange={(event) => setName(event.target.value)} placeholder="party_parrot" value={name} />
      <TextField
        accept="image/png,image/gif,image/webp"
        error={fileError}
        label="Image"
        onChange={(event) => setFile(event.target.files?.[0] ?? null)}
        type="file"
      />
    </Dialog>
  )
}
