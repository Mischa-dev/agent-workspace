import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp, opKey, opPrefix } from '../api/ops'
import { Button, ConfirmDialog, ErrorState, ListSkeleton, StatusChip, describeError, showToast } from '../ui'
import { refreshAfterTemplate } from './api'
import './templates.css'

// M1.17: what a workspace is for. First run asks it; Settings › Modules can
// change it later. A shape turns modules on and off, sets the starting
// layout and adds starter channels and pages; it never deletes anything.

type Changing = { id: string; name: string; changes: string[] }

export function ShapePicker({ canChange, organizationId }: { canChange: boolean; organizationId: string }) {
  const queryClient = useQueryClient()
  const input = { shapes: true }
  const shapes = useQuery({
    queryKey: opKey('directory_list', input, organizationId),
    queryFn: () => callOp('directory_list', input, { workspaceId: organizationId }),
  })
  const [asking, setAsking] = useState<Changing | null>(null)
  // What setting it would do: the operation run and rolled back.
  const preview = useMutation({
    mutationFn: (shape: { id: string; name: string }) =>
      callOp('workspace_shape_set', { shape: shape.id }, { workspaceId: organizationId, preview: true }),
    onSuccess: (result, shape) => setAsking({ id: shape.id, name: shape.name, changes: result.installed.changes }),
  })
  const set = useMutation({
    mutationFn: (shape: Changing) => callOp('workspace_shape_set', { shape: shape.id }, { workspaceId: organizationId }),
    onSuccess: (result) => {
      setAsking(null)
      refreshAfterTemplate(queryClient, organizationId)
      void queryClient.invalidateQueries({ queryKey: opPrefix('directory_list', organizationId) })
      showToast({
        tone: 'success',
        title: `Set up as ${result.name}`,
        description: result.installed.changes.length ? result.installed.changes.join(' ') : 'The workspace already matched it.',
      })
    },
  })

  if (shapes.isPending) return <ListSkeleton label="Loading shapes" rows={2} />
  if (shapes.isError) return <ErrorState error={shapes.error} onRetry={() => void shapes.refetch()} title="Shapes didn’t load" />
  const current = shapes.data.shape

  return (
    <>
      <ul aria-label="What this workspace is for" className="aw-templates">
        {shapes.data.entries.map((shape) => (
          <li className="aw-card aw-templates__card" key={shape.id}>
            <div className="aw-templates__head">
              <h3 className="aw-templates__name">{shape.name}</h3>
              {current === shape.id ? <StatusChip tone="success">This workspace</StatusChip> : null}
            </div>
            <p className="aw-templates__description">{shape.description}</p>
            {canChange ? (
              <div className="aw-templates__actions">
                <Button
                  disabled={preview.isPending || set.isPending}
                  onClick={() => preview.mutate({ id: shape.id, name: shape.name })}
                  size="sm"
                  variant={current ? undefined : shape.id === 'shapes.team' ? 'primary' : undefined}
                >
                  {preview.isPending && preview.variables?.id === shape.id ? 'Checking…' : `Choose ${shape.name}`}
                </Button>
              </div>
            ) : null}
          </li>
        ))}
      </ul>
      {preview.isError ? <p className="aw-templates__description">{describeError(preview.error).message}</p> : null}
      <ConfirmDialog
        busy={set.isPending}
        confirmLabel={asking ? `Set up as ${asking.name}` : 'Set up'}
        error={set.isError ? describeError(set.error).message : undefined}
        onClose={() => {
          setAsking(null)
          set.reset()
        }}
        onConfirm={() => asking && set.mutate(asking)}
        open={Boolean(asking)}
        title={asking ? `Set up as ${asking.name}?` : 'Change what this workspace is for?'}
        tone="primary"
      >
        {asking?.changes.length ? asking.changes.join(' ') : 'The workspace already matches it.'} Nothing is deleted: a module
        turned off is only hidden, and you can change this again any time.
      </ConfirmDialog>
    </>
  )
}
