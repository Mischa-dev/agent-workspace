// Move a page to another space (C1.37): it keeps its versions, comments and
// files, links to it keep working, and who sees it is the new space's.
import { useState, type FormEvent } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router-dom'

import { callOp, newRequestId } from '../api/ops'
import { useProjects } from '../projects/hooks'
import { projectLocationHref } from '../projects/locations'
import { Banner, Button, Dialog, SelectField, describeError, showToast } from '../ui'
import { memoryKeys } from './keys'

export function MovePageDialog({
  onClose,
  page,
  projectId,
}: {
  onClose: () => void
  page: { id: string; title: string; version: number }
  /** The space the page is in now. */
  projectId: string
}) {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const projects = useProjects()
  const choices = (projects.data?.projects ?? []).filter((project) => project.id !== projectId && !project.archivedAt)
  const [target, setTarget] = useState('')
  const chosen = target || choices[0]?.id || ''
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [requestId] = useState(() => newRequestId())

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!chosen || busy) return
    setBusy(true)
    setError(null)
    void callOp('page_move', { pageId: page.id, toSpaceId: chosen, expectedVersion: page.version }, { requestId })
      .then(async (moved) => {
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: memoryKeys.pages(projectId) }),
          queryClient.invalidateQueries({ queryKey: memoryKeys.pages(moved.spaceId) }),
        ])
        const name = choices.find((project) => project.id === moved.spaceId)?.name
        showToast({ tone: 'success', title: name ? `Moved to ${name}` : 'Page moved' })
        onClose()
        navigate(projectLocationHref(moved.spaceId, { kind: 'page', pageId: moved.id }), { replace: true })
      })
      .catch((reason: unknown) => setError(`The page wasn’t moved. ${describeError(reason).message}`))
      .finally(() => setBusy(false))
  }

  return (
    <Dialog
      description="It keeps its versions, comments and files, and links to it keep working. Who can see it becomes the new project’s members."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button busy={busy} disabled={!chosen} type="submit" variant="primary">
            Move page
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title={`Move “${page.title}”`}
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
      {projects.isPending ? null : choices.length ? (
        <SelectField label="Project" onChange={(event) => setTarget(event.target.value)} value={chosen}>
          {choices.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name}
            </option>
          ))}
        </SelectField>
      ) : (
        <Banner tone="info">There’s no other project to move it to.</Banner>
      )}
    </Dialog>
  )
}
