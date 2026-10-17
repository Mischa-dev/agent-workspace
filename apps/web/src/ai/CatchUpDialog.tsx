import { useState, type RefObject } from 'react'
import { useQuery } from '@tanstack/react-query'

import { Button, Dialog, ErrorState, MessageSkeleton, SegmentedControl } from '../ui'
import { summarizeConversation } from './api'
import { aiErrorMessage } from './links'
import { SummaryView } from './SummaryView'

type Range = 'all' | 'day'

/** A few lines on what a thread decided and where it stands, from the messages the reader can see. */
export default function CatchUpDialog({
  conversationId,
  name,
  onClose,
  returnFocus,
}: {
  conversationId: string
  name: string
  onClose: () => void
  returnFocus: RefObject<HTMLElement | null>
}) {
  const [range, setRange] = useState<Range>('all')
  // Fixed when the dialog opens, so switching back and forth reuses the answer.
  const [since] = useState(() => new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString())
  const summary = useQuery({
    queryKey: ['conversation', conversationId, 'ai-summary', range],
    queryFn: () => summarizeConversation(conversationId, range === 'day' ? since : null),
    staleTime: 5 * 60_000,
    retry: false,
  })
  return (
    <Dialog
      footer={<Button onClick={onClose}>Close</Button>}
      onClose={onClose}
      open
      returnFocus={returnFocus}
      title={`Catch up on ${name}`}
      width={560}
    >
      <SegmentedControl<Range>
        label="How far back"
        onChange={setRange}
        options={[
          { value: 'all', label: 'Whole thread' },
          { value: 'day', label: 'Last 24 hours' },
        ]}
        value={range}
      />
      <div aria-live="polite">
        {summary.isPending ? <MessageSkeleton label="Summarizing" rows={2} /> : null}
        {summary.isError ? (
          <ErrorState action={<Button onClick={() => void summary.refetch()}>Try again</Button>} title="No summary this time">
            {aiErrorMessage(summary.error, 'Summarizing')}
          </ErrorState>
        ) : null}
        {summary.data ? <SummaryView summary={summary.data.summary} /> : null}
      </div>
    </Dialog>
  )
}
