import { useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'

import { useProjectExperience } from '../experience/layout'
import { useFields } from '../fields/api'
import type { FieldDefinition, FieldValue } from '../fields/fieldModel'
import { LazyLaunchDialog as LaunchDialog } from '../launch/LazyLaunchDialog'
import { launchSummary, launchesHref } from '../launch/launchModel'
import { countLabel } from '../ui'
import { ComboboxList, Popover } from '../ui/pickers'
import { BulkBar } from './BulkBar'
import { useCardFields } from './cardFields'
import type { PickerField } from './FieldPicker'
import { FieldEditor } from '../fields/FieldEditor'
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
  const fields = useCardFields(projectId)
  const experience = useProjectExperience(projectId)
  const launch = experience.shows('task.runs')
  const projectFields = useFields({ projectId })
  const customFields = experience.shows('task.custom_fields') ? (projectFields.data?.fields ?? []).filter((field) => !field.archived) : []
  const navigate = useNavigate()
  const [saving, setSaving] = useState(false)
  const [launching, setLaunching] = useState<WorkItemCard[] | null>(null)
  // Custom fields (AWD-101): pick "Set" or "Clear" for a field, then set
  // it in the field's own editor. The chooser lists actions, not values, so
  // none of its options is ever selected.
  const [choosing, setChoosing] = useState<HTMLElement | null>(null)
  const [editing, setEditing] = useState<{ field: FieldDefinition; anchor: HTMLElement } | null>(null)
  // A multiple choice reports each toggle; the last one is sent when its editor closes.
  const chosen = useRef<{ value: FieldValue | null } | null>(null)

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

  const setField = async (field: FieldDefinition, value: FieldValue | null) => {
    const cards = [...selected]
    if (!(await run({ fieldValues: { [field.id]: value } }, cards))) return
    notify({ tone: 'success', title: value === null ? `Cleared ${field.name} on ${countLabel(cards.length, 'task')}` : `Set ${field.name} on ${countLabel(cards.length, 'task')}` })
  }

  const chooseField = (choice: string) => {
    const anchor = choosing
    setChoosing(null)
    const [mode, id] = choice.split(':')
    const field = customFields.find((entry) => entry.id === id)
    if (!field || !anchor) return
    if (mode === 'clear') return void setField(field, null)
    // A checkbox has one value to set.
    if (field.type === 'checkbox') return void setField(field, true)
    chosen.current = null
    setEditing({ field, anchor })
  }

  const finishEditing = () => {
    const done = editing
    setEditing(null)
    const value = chosen.current
    chosen.current = null
    // "Set" never clears: a selection toggled back to empty, or an editor
    // closed without a choice, changes nothing ("Clear" is its own entry).
    if (done && value && value.value !== null) void setField(done.field, value.value)
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
        fields={fields}
        onArchive={() => void archive()}
        onClear={onClear}
        onDueDate={(value) => void setDueDate(value)}
        onField={customFields.length ? setChoosing : undefined}
        onLaunch={launch ? () => setLaunching(selected.filter((card) => !card.archivedAt)) : undefined}
        onPick={onPick}
        onRestore={() => void restore()}
      />
      {choosing ? (
        <Popover anchor={choosing} label="Change a field on the selected tasks" onClose={() => setChoosing(null)} open width={300}>
          <ComboboxList
            label="Field"
            onClose={() => setChoosing(null)}
            onSelect={chooseField}
            options={customFields.flatMap((field) => [
              { value: `set:${field.id}`, label: `Set ${field.name}`, keywords: field.name },
              { value: `clear:${field.id}`, label: `Clear ${field.name}`, keywords: field.name },
            ])}
            placeholder="Find a field"
            selected={[]}
          />
        </Popover>
      ) : null}
      {editing ? (
        <FieldEditor
          anchor={editing.anchor}
          field={editing.field}
          onClose={finishEditing}
          onSet={(value) => {
            chosen.current = { value }
          }}
          projectId={projectId}
          references={[]}
          taskId=""
          value={undefined}
        />
      ) : null}
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
