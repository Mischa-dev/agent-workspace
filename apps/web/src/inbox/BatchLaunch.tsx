import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { toastError } from '../app/mutations'
import { Button, Icon, showToast } from '../ui'
import { launchBatch, usageKeys } from '../settings/account/usageApi'
import { batchOutcome, launchLabel, newAttemptKey } from '../settings/account/usageModel'
import type { InboxNotification } from './api'

/**
 * A capacity alert's actions (U4): Launch starts the suggested batch in one
 * tap, each task in its own project's environment on the alert's account;
 * Review opens the batch on the usage page to edit it first.
 */
export function BatchLaunch({ batch, reviewHref }: { batch: NonNullable<InboxNotification['batch']>; reviewHref: string | null }) {
  const queryClient = useQueryClient()
  // One key per alert while it's shown, so a double tap launches once.
  const [attempt] = useState(() => newAttemptKey('alert-batch'))
  const launch = useMutation({
    mutationFn: () => launchBatch(batch.accountId, { windowKey: batch.windowKey }, attempt),
    onSuccess: (result) => {
      showToast(result.launched === 0 && result.failed.length === 0 ? { tone: 'info', title: 'Already launched', description: 'Nothing new was ready to add.' } : batchOutcome(result))
      void queryClient.invalidateQueries({ queryKey: usageKeys.root })
    },
    onError: (reason) => toastError('The batch didn’t launch', reason),
  })
  return (
    <>
      <Button busy={launch.isPending} disabled={launch.isSuccess} icon="play" onClick={() => launch.mutate()} variant="primary">
        {launch.isSuccess ? 'Launched' : launchLabel(batch.size)}
      </Button>
      {reviewHref ? (
        <Link className="aw-btn aw-btn--secondary aw-btn--md" to={reviewHref}>
          <Icon name="list" size={16} />
          <span className="aw-btn__label">Review</span>
        </Link>
      ) : null}
    </>
  )
}
