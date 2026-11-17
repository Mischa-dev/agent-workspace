import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useId } from 'react'

import { opQuery, useOp } from '../api/ops'
import type { FieldDefinition } from '../fields/fieldModel'
import { Button, describeError, showToast } from '../ui'
import { collectionKeys } from './api'

/**
 * A button field on one record (T1.21): pressing it runs the button's
 * operations as whoever presses it, with the record filled in. When they
 * can't run them here, it is drawn disabled with the reason beside it, for
 * the pointer and for screen readers.
 */
export function RecordButton({
  collectionId,
  field,
  projectId,
  record,
  size = 'sm',
}: {
  collectionId: string
  field: FieldDefinition
  projectId: string
  record: { number: number; title: string }
  size?: 'sm' | 'md'
}) {
  const reasonId = useId()
  const queryClient = useQueryClient()
  // One answer per button, shared by every row.
  const access = useQuery({ ...opQuery('button_get', { collection: collectionId, field: field.id }), staleTime: 60_000 })
  const press = useOp('button_press', {
    onSuccess: () => {
      showToast({ title: `${label} done on #${record.number}` })
      // What the steps changed may show in this collection.
      void queryClient.invalidateQueries({ queryKey: collectionKeys.one(projectId, collectionId) })
    },
  })
  const label = field.button?.label || field.name
  const reason = access.data && !access.data.canPress ? (access.data.reason ?? 'You can’t press this here.') : null
  return (
    <span className="aw-recordbutton" title={reason ?? undefined}>
      <Button
        aria-describedby={reason ? reasonId : undefined}
        busy={press.isPending}
        disabled={!access.data || Boolean(reason)}
        onClick={(event) => {
          event.stopPropagation()
          press.mutate(
            { collection: collectionId, field: field.id, record: `#${record.number}` },
            { onError: (error) => showToast({ title: `${label} didn’t go through`, description: describeError(error).message, tone: 'danger' }) },
          )
        }}
        size={size}
        variant="secondary"
      >
        {label}
      </Button>
      {reason ? (
        <span className="aw-sr-only" id={reasonId}>
          {reason}
        </span>
      ) : null}
    </span>
  )
}
