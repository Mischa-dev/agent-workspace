import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { callOp, opKey, opPrefix, type OpOutput } from '../../api/ops'
import { Button, ConfirmDialog, ErrorState, ListSkeleton, Row, RowList, StatusChip, describeError, showToast } from '../../ui'
import { refreshAfterTemplate } from '../../workspace-templates/api'

// M1.11: Settings › Modules › Available. What the directory offers this
// workspace (built in, the server's folder and lists, the workspace's own
// lists and saved templates), installed after a preview of what it does.
// An entry that couldn't be read shows why.

type Entry = OpOutput<'directory_list'>['entries'][number]
type Plan = OpOutput<'package_preview'>

function sourceLabel(entry: Entry) {
  switch (entry.source) {
    case 'built_in':
      return 'Built in'
    case 'saved_template':
      return 'Saved here'
    case 'workspace':
      return 'This workspace’s list'
    default:
      return 'This server'
  }
}

/** The plan in plain words: what it makes, reuses, changes and leaves out. */
function planWords(plan: Plan) {
  const count = (action: string) => plan.items.filter((item) => item.action === action).length
  const words = [
    count('create') ? `Makes ${count('create')} new part${count('create') === 1 ? '' : 's'}.` : '',
    count('reuse') ? `Reuses ${count('reuse')} already here.` : '',
    count('update') ? `Changes ${count('update')} (roles, modules or the layout).` : '',
    count('skip') ? `Leaves out ${count('skip')}.` : '',
    plan.turnsOff.length ? `Turns off ${plan.turnsOff.join(', ')}.` : '',
    plan.turnsOn.length ? `Turns on ${plan.turnsOn.join(', ')}.` : '',
  ]
  return words.filter(Boolean).join(' ') || 'Nothing would change.'
}

export function AvailablePackages({ canManage, organizationId }: { canManage: boolean; organizationId: string }) {
  const queryClient = useQueryClient()
  const input = { shapes: false }
  const directory = useQuery({
    queryKey: opKey('directory_list', input, organizationId),
    queryFn: () => callOp('directory_list', input, { workspaceId: organizationId }),
  })
  const [asking, setAsking] = useState<{ entry: Entry; plan: Plan } | null>(null)
  const preview = useMutation({
    mutationFn: (entry: Entry) => callOp('package_preview', { entry: entry.entry }, { workspaceId: organizationId }),
    onSuccess: (plan, entry) => setAsking({ entry, plan }),
  })
  const install = useMutation({
    mutationFn: ({ entry, plan }: { entry: Entry; plan: Plan }) =>
      callOp('package_install', { entry: entry.entry, planHash: plan.planHash }, { workspaceId: organizationId }),
    onSuccess: (installed, { entry }) => {
      setAsking(null)
      refreshAfterTemplate(queryClient, organizationId)
      void queryClient.invalidateQueries({ queryKey: opPrefix('directory_list', organizationId) })
      showToast({
        tone: 'success',
        title: `${entry.name} installed`,
        description: installed.changes.length ? installed.changes.join(' ') : 'Everything it brings was already here.',
      })
    },
  })

  if (directory.isPending) return <ListSkeleton label="Loading what can be installed" rows={3} />
  if (directory.isError) {
    return <ErrorState error={directory.error} onRetry={() => void directory.refetch()} title="The directory didn’t load" />
  }

  return (
    <>
      <RowList label="Packages that can be installed">
        {directory.data.entries.map((entry) => (
          <Row
            actions={
              entry.error ? (
                <StatusChip tone="danger">Can’t be read</StatusChip>
              ) : canManage ? (
                <Button
                  disabled={preview.isPending || install.isPending}
                  onClick={() => preview.mutate(entry)}
                  size="sm"
                >
                  {preview.isPending && preview.variables?.entry === entry.entry ? 'Checking…' : 'Install'}
                </Button>
              ) : null
            }
            key={`${entry.source}:${entry.entry || entry.id}:${entry.version}`}
            meta={
              <>
                {entry.error ?? entry.description} <span className="aw-muted">{sourceLabel(entry)}</span>
              </>
            }
            title={entry.version ? `${entry.name} ${entry.version}` : entry.name}
          />
        ))}
      </RowList>
      {preview.isError ? <p role="alert">{describeError(preview.error).message}</p> : null}
      <ConfirmDialog
        busy={install.isPending}
        confirmLabel={asking ? `Install ${asking.entry.name}` : 'Install'}
        error={install.isError ? describeError(install.error).message : undefined}
        onClose={() => {
          setAsking(null)
          install.reset()
        }}
        onConfirm={() => asking && install.mutate(asking)}
        open={Boolean(asking)}
        title={asking ? `Install ${asking.entry.name}?` : 'Install this package?'}
        tone="primary"
      >
        {asking ? planWords(asking.plan) : null} Installing never deletes anything: what’s already here with the same name is
        reused.
      </ConfirmDialog>
    </>
  )
}
