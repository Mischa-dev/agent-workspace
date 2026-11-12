import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { toastError } from '../app/toastError'
import { Button, ConfirmDialog, ErrorState, ListSkeleton, StatusChip, describeError, showToast } from '../ui'
import {
  applyTemplate,
  listTemplates,
  refreshAfterTemplate,
  removeTemplate,
  templateKeys,
  type Applied,
  type WorkspaceTemplate,
} from './api'
import './templates.css'

/**
 * AWD-76: the workspace's templates as cards, Everything first, each saying
 * in plain words what using it does. Setup uses one straight away; settings
 * asks first, since the workspace already has its own setup by then.
 */
export function TemplatePicker({
  confirm,
  onApplied,
  organizationId,
}: {
  confirm: boolean
  onApplied?: (applied: Applied) => void
  organizationId: string
}) {
  const queryClient = useQueryClient()
  const templates = useQuery({ queryKey: templateKeys.workspace(organizationId), queryFn: () => listTemplates(organizationId) })
  const [asking, setAsking] = useState<WorkspaceTemplate | null>(null)
  const [removing, setRemoving] = useState<WorkspaceTemplate | null>(null)
  const apply = useMutation({
    mutationFn: (template: WorkspaceTemplate) => applyTemplate(organizationId, template.id),
    onSuccess: (applied) => {
      setAsking(null)
      refreshAfterTemplate(queryClient, organizationId)
      showToast({
        tone: 'success',
        title: `Using ${applied.templateName}`,
        description: applied.changes.length ? applied.changes.join(' ') : 'The workspace already matched it, so nothing changed.',
      })
      onApplied?.(applied)
    },
    onError: (error) => {
      if (!confirm) toastError('The template wasn’t used', error)
    },
  })
  const remove = useMutation({
    mutationFn: (template: WorkspaceTemplate) => removeTemplate(organizationId, template.id),
    onSuccess: (_, template) => {
      setRemoving(null)
      void queryClient.invalidateQueries({ queryKey: templateKeys.workspace(organizationId) })
      showToast({ title: `${template.name} removed` })
    },
  })

  if (templates.isPending) return <ListSkeleton label="Loading templates" rows={3} />
  if (templates.isError) {
    return <ErrorState error={templates.error} onRetry={() => void templates.refetch()} title="Templates didn’t load" />
  }
  const { canUse, lastUsed } = templates.data
  const use = (template: WorkspaceTemplate) => (confirm ? setAsking(template) : apply.mutate(template))

  return (
    <>
      <ul aria-label="Workspace templates" className="aw-templates">
        {templates.data.templates.map((template) => (
          <li className="aw-card aw-templates__card" key={template.id}>
            <div className="aw-templates__head">
              <h3 className="aw-templates__name">{template.name}</h3>
              {lastUsed?.template === template.id ? <StatusChip tone="success">Last used</StatusChip> : null}
              {template.builtIn ? null : <StatusChip>Saved here</StatusChip>}
            </div>
            {template.description ? <p className="aw-templates__description">{template.description}</p> : null}
            {template.whatItDoes.length ? (
              <ul className="aw-templates__does">
                {template.whatItDoes.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            ) : null}
            {canUse ? (
              <div className="aw-templates__actions">
                <Button
                  disabled={apply.isPending}
                  onClick={() => use(template)}
                  size="sm"
                  variant={template.id === 'everything' ? 'primary' : undefined}
                >
                  {apply.isPending && apply.variables?.id === template.id ? 'Setting up…' : `Use ${template.name}`}
                </Button>
                {template.builtIn ? null : (
                  <Button onClick={() => setRemoving(template)} size="sm" variant="ghost">
                    Remove
                  </Button>
                )}
              </div>
            ) : null}
          </li>
        ))}
      </ul>
      <ConfirmDialog
        busy={apply.isPending}
        confirmLabel={asking ? `Use ${asking.name}` : 'Use'}
        error={apply.isError ? describeError(apply.error).message : undefined}
        onClose={() => {
          setAsking(null)
          apply.reset()
        }}
        onConfirm={() => asking && apply.mutate(asking)}
        open={Boolean(asking)}
        title={asking ? `Use ${asking.name}?` : 'Use this template?'}
        tone="primary"
      >
        {asking?.whatItDoes.join(' ')} Nothing is deleted: modules it turns off are only hidden, and roles, projects and channels
        that are already here stay as they are.
      </ConfirmDialog>
      <ConfirmDialog
        busy={remove.isPending}
        confirmLabel="Remove"
        error={remove.isError ? describeError(remove.error).message : undefined}
        onClose={() => {
          setRemoving(null)
          remove.reset()
        }}
        onConfirm={() => removing && remove.mutate(removing)}
        open={Boolean(removing)}
        title={removing ? `Remove ${removing.name}?` : 'Remove this template?'}
      >
        Nothing it set up changes. Only the saved template goes.
      </ConfirmDialog>
    </>
  )
}
