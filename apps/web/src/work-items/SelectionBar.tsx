import { useState } from 'react'
import { useNavigate } from 'react-router-dom'

import { LazyLaunchDialog as LaunchDialog } from '../launch/LazyLaunchDialog'
import { launchSummary, launchesHref } from '../launch/launchModel'
import { countLabel } from '../ui'
import { BulkBar } from './BulkBar'
import type { PickerField } from './FieldPicker'
import type { Notice } from './notice'
import { bulkUpdateWorkItems } from './planningApi'
import { useInvalidateWorkItems } from './planningHooks'
import { editErrorMessage } from './taskEdits'
import type { BulkChanges, WorkItemCard } from './types'

/**
 * The bar for the selected tasks on the list and the board: pickers, due
 * date, launch, archive and restore. Every selected task changes, or none do.
 */
export function SelectionBar({
  busy,
  notify,
  onClear,
  onPick,
  projectId,
  selected,
}: {
  /** A picker's change is still saving. */
  busy: boolean
  notify: (notice: Notice) => void
  onClear: () => void
  onPick: (field: PickerField, anchor: HTMLElement) => void
  projectId: string
  selected: readonly WorkItemCard[]
}) {
  const invalidate = useInvalidateWorkItems()
  const navigate = useNavigate()
  const [saving, setSaving] = useState(false)
  const [launching, setLaunching] = useState<WorkItemCard[] | null>(null)

  const run = async (changes: BulkChanges, cards: readonly WorkItemCard[]) => {
    setSaving(true)
    try {
      return await bulkUpdateWorkItems({
        projectId,
        items: cards.map((card) => ({ id: card.id, expectedVersion: card.version })),
        changes,
        idempotencyKey: crypto.randomUUID(),
      })
    } catch (error) {
      notify({ tone: 'danger', title: cards.length === 1 ? `${cards[0].key} didn’t change` : `${countLabel(cards.length, 'task')} didn’t change`, description: editErrorMessage(error, 'The change') })
      return null
    } finally {
      setSaving(false)
      void invalidate(projectId)
    }
  }

  const archive = async () => {
    const cards = [...selected]
    const result = await run({ archived: true }, cards)
    if (!result) return
    onClear()
    notify({ tone: 'success', title: `Archived ${countLabel(cards.length, 'task')}`, onUndo: () => run({ archived: false }, result.items) })
  }

  const restore = async () => {
    const cards = [...selected]
    if (!(await run({ archived: false }, cards))) return
    onClear()
    notify({ tone: 'success', title: `Restored ${countLabel(cards.length, 'task')}` })
  }

  const setDueDate = async (value: string | null) => {
    const cards = [...selected]
    if (!(await run({ dueDate: value }, cards))) return
    notify({ tone: 'success', title: value ? `Changed the due date of ${countLabel(cards.length, 'task')}` : `Cleared the due date of ${countLabel(cards.length, 'task')}` })
  }

  return (
    <>
      <BulkBar
        archived={selected.length > 0 && selected.every((card) => Boolean(card.archivedAt))}
        busy={saving || busy}
        count={selected.length}
        onArchive={() => void archive()}
        onClear={onClear}
        onDueDate={(value) => void setDueDate(value)}
        onLaunch={() => setLaunching(selected.filter((card) => !card.archivedAt))}
        onPick={onPick}
        onRestore={() => void restore()}
      />
      {launching ? (
        <LaunchDialog
          onClose={() => setLaunching(null)}
          onLaunched={(launches) => {
            setLaunching(null)
            onClear()
            notify({ ...launchSummary(launches), action: { label: 'View runs', run: () => navigate(launchesHref(projectId)) } })
          }}
          projectId={projectId}
          tasks={launching.map((card) => ({ id: card.id, key: card.key, title: card.title }))}
        />
      ) : null}
    </>
  )
}
