// Make a page (C1.25): a page from picked messages, a whole thread, or some
// of another page's blocks (copied, or moved with an embed left behind).
// The new page remembers what it was made from.
import { useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { callOp, newRequestId } from '../api/ops'
import type { Project } from '../projects/types'
import { projectLocationHref } from '../projects/locations'
import { Banner, Button, Checkbox, Dialog, SelectField, TextField, describeError, showToast } from '../ui'
import { memoryKeys } from './keys'

/** What a page is made from: exactly one of these. */
export type PageSource =
  | { kind: 'messages'; messageIds: string[] }
  | { kind: 'thread'; threadId: string }
  | { kind: 'blocks'; pageId: string; blockIds: string[] }

export function MakePageDialog({
  onClose,
  projectId,
  projects = null,
  source,
  suggestedTitle = '',
}: {
  onClose: () => void
  /** The space the source is in. */
  projectId: string
  /** For a DM or group: the spaces the page can go in. */
  projects?: Project[] | null
  source: PageSource
  suggestedTitle?: string
}) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const [title, setTitle] = useState(suggestedTitle)
  const [target, setTarget] = useState(() => projects?.[0]?.id ?? projectId)
  const [move, setMove] = useState(false)
  // A line in the conversation that says the messages became a page.
  const [saySo, setSaySo] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [requestId] = useState(() => newRequestId())

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!target || busy) return
    setBusy(true)
    setError(null)
    const from =
      source.kind === 'messages'
        ? { messages: source.messageIds, saySo }
        : source.kind === 'thread'
          ? { threadId: source.threadId, saySo }
          : { pageId: source.pageId, blocks: source.blockIds, move }
    void callOp('page_make', { spaceId: target, title: title.trim() || null, ...from }, { requestId })
      .then(async (made) => {
        await queryClient.invalidateQueries({ queryKey: memoryKeys.pages(made.spaceId) })
        if (source.kind === 'blocks' && move) {
          await queryClient.invalidateQueries({ queryKey: memoryKeys.page(projectId, source.pageId) })
        }
        const href = projectLocationHref(made.spaceId, { kind: 'page', pageId: made.id })
        showToast({
          tone: 'success',
          title: move ? `Moved to “${made.title}”` : `Made “${made.title}”`,
          action: { label: 'Open', onSelect: () => navigate(href) },
        })
        onClose()
      })
      .catch((reason: unknown) => setError(`The page wasn’t made. ${describeError(reason).message}`))
      .finally(() => setBusy(false))
  }

  const description =
    source.kind === 'blocks'
      ? 'The blocks you picked become a new page, which links back here.'
      : projects
        ? 'The messages are quoted on a page in a project you pick. Its members will see the page, but not this conversation.'
        : 'The messages are quoted on a new page that links back to them.'

  return (
    <Dialog
      description={description}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} disabled={!target} type="submit" variant="primary">
            {move ? 'Move to page' : 'Make page'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title="Make a page"
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      {projects ? (
        <SelectField label="Project" onChange={(event) => setTarget(event.target.value)} value={target}>
          {projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name}
            </option>
          ))}
        </SelectField>
      ) : null}
      <TextField
        hint="Leave it empty to use the first words."
        label="Title"
        maxLength={200}
        onChange={(event) => setTitle(event.target.value)}
        value={title}
      />
      {source.kind === 'blocks' ? (
        <Checkbox checked={move} label="Move them, and leave the new page embedded here" onChange={setMove} />
      ) : (
        <Checkbox checked={saySo} label="Say so in the conversation" onChange={setSaySo} />
      )}
    </Dialog>
  )
}
