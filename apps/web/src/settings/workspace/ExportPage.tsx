import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { toastError } from '../../app/mutations'
import { callOp, opKey, opQuery } from '../../api/ops'
import type { TransferOutput } from '../../api/generated/operations'
import { formatBytes } from '../../files/model'
import { Button, EmptyState, ErrorState, ListSkeleton, PageHeader, Row, RowList, Section, StatusChip, formatDateTime, showToast } from '../../ui'
import type { SettingsPageProps } from '../types'

// H1.9: the whole workspace as one archive, to keep or to bring to another
// server. Only the person who asked can download it, for seven days.

const STATE_WORDS: Record<string, string> = {
  queued: 'Waiting',
  running: 'Writing',
  succeeded: 'Ready',
  failed: 'Failed',
  expired: 'Expired',
}

const working = (transfer: TransferOutput) => transfer.state === 'queued' || transfer.state === 'running'

function TransferRow({ transfer }: { transfer: TransferOutput }) {
  const ready = transfer.state === 'succeeded' && transfer.downloadUrl
  return (
    <Row
      actions={
        ready ? (
          <a className="aw-btn aw-btn--secondary aw-btn--sm" download href={transfer.downloadUrl ?? undefined}>
            <span className="aw-btn__label">Download</span>
          </a>
        ) : null
      }
      meta={
        <>
          {formatDateTime(transfer.createdAt)}
          {transfer.byteSize ? ` · ${formatBytes(transfer.byteSize)}` : ''}
          {ready && transfer.expiresAt ? ` · until ${formatDateTime(transfer.expiresAt)}` : ''}
        </>
      }
      title={transfer.direction === 'import' ? 'Import' : 'Export'}
    >
      <StatusChip tone={transfer.state === 'failed' ? 'danger' : ready ? 'success' : 'muted'}>{STATE_WORDS[transfer.state] ?? transfer.state}</StatusChip>
      {transfer.error ? <p className="aw-pagesection__desc">{transfer.error}</p> : null}
    </Row>
  )
}

export default function ExportPage({ workspace }: SettingsPageProps) {
  const queryClient = useQueryClient()
  const workspaceId = workspace?.id
  const transfers = useQuery({
    ...opQuery('workspace_transfer_list', {}, { workspaceId }),
    enabled: Boolean(workspaceId),
    refetchInterval: (query) => (query.state.data?.transfers.some(working) ? 2000 : false),
  })
  const start = useMutation({
    mutationFn: () => callOp('workspace_export_start', {}, { workspaceId }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: opKey('workspace_transfer_list', {}, workspaceId) })
      showToast({ title: 'Export started', description: 'It appears here to download when it’s written.' })
    },
    onError: (error) => toastError('The export didn’t start', error),
  })
  if (!workspace) return null
  const list = transfers.data?.transfers ?? []
  return (
    <>
      <PageHeader icon="archive" title="Export">
        All of {workspace.name} as one archive: its spaces, conversations, tasks, pages, records and files, with their
        history. Direct messages and group conversations stay with the people in them, and secrets go by name only.
      </PageHeader>
      <Section
        actions={
          <Button busy={start.isPending} disabled={list.some(working)} onClick={() => start.mutate()} variant="primary">
            Export the workspace
          </Button>
        }
        description="Only you can download an export you start, for seven days. Another server can import it, as a new workspace or on its first run."
        id="exports-heading"
        title="Archives"
      >
        {transfers.isPending ? (
          <ListSkeleton label="Loading exports" rows={3} />
        ) : transfers.isError ? (
          <ErrorState error={transfers.error} onRetry={() => void transfers.refetch()} title="Exports didn’t load" />
        ) : list.length === 0 ? (
          <EmptyState icon="archive" title="No exports yet">
            Start one to keep a copy or move the workspace.
          </EmptyState>
        ) : (
          <RowList label="Exports and imports">
            {list.map((transfer) => (
              <TransferRow key={transfer.id} transfer={transfer} />
            ))}
          </RowList>
        )}
      </Section>
    </>
  )
}
