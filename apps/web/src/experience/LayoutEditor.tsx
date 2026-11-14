import { useId, useMemo, useState, type FormEvent, type ReactNode } from 'react'

import { Banner, Button, Dialog, Icon, Switch, TextArea, TextField, type IconGlyph } from '../ui'
import type { Word } from './layout'
import type { NounId, PartId } from './parts'
import { presetChoices, type PresetChoice } from './presetChoices'
import { REGISTRY, everything, layoutFrom, layoutShape, presetShape, shapeWord, type Layout, type SavedPreset, type Shape } from './registry'
import './layout.css'
import '../styles/pages.css'

// The pieces both Layout settings pages use: preset cards with a small
// picture of the sidebar each gives, switches by group, words, a live
// preview and the save bar.

type Row = { part: PartId; icon: IconGlyph; label: string }

/** The rows a shape puts at the top of a project's sidebar, lead parts first. */
function topRows(shape: Shape): Row[] {
  const rows: Row[] = []
  if (shape.visible.has('area.tasks')) rows.push({ part: 'area.tasks', icon: 'work', label: capital(shapeWord(shape, 'task', 'many')) })
  if (shape.visible.has('area.pages')) rows.push({ part: 'area.pages', icon: 'list', label: capital(shapeWord(shape, 'page', 'many')) })
  else if (shape.visible.has('area.decisions')) rows.push({ part: 'area.pages', icon: 'list', label: 'Decisions' })
  if (shape.visible.has('area.runs')) rows.push({ part: 'area.runs', icon: 'activity', label: 'Runs' })
  const first = (row: Row) => (shape.lead.includes(row.part) ? 0 : 1)
  return rows.sort((left, right) => first(left) - first(right))
}

const capital = (text: string) => (text ? text.charAt(0).toLocaleUpperCase() + text.slice(1) : text)

/** A card's small picture of the sidebar its preset gives. */
export function MiniPreview({ shape }: { shape: Shape }) {
  return (
    <div aria-hidden="true" className="aw-minipreview" data-region="sidebar">
      <span className="aw-minipreview__rows">
        <span className="aw-minipreview__row">
          <Icon name="home" size={12} />
          <span>Overview</span>
        </span>
        {topRows(shape).map((row) => (
          <span className="aw-minipreview__row" key={row.part}>
            <Icon name={row.icon} size={12} />
            <span>{row.label}</span>
          </span>
        ))}
        {shape.visible.has('area.agents') ? (
          <span className="aw-minipreview__row">
            <Icon name="bot" size={12} />
            <span>Agents</span>
          </span>
        ) : null}
        {shape.visible.has('area.chat') ? (
          <>
            <span className="aw-minipreview__head">{shapeWord(shape, 'channel', 'many')}</span>
            <span className="aw-minipreview__row">
              <Icon name="hash" size={12} />
              <span>general</span>
            </span>
          </>
        ) : null}
      </span>
    </div>
  )
}

/** Cards to pick a preset from, as radio buttons: arrow keys move between them. */
export function PresetCards({
  choices,
  disabled = false,
  label,
  onChange,
  value,
}: {
  choices: PresetChoice[]
  disabled?: boolean
  label: string
  onChange: (id: string | null) => void
  value: string | null
}) {
  const name = useId()
  return (
    <div aria-label={label} className="aw-presets" role="radiogroup">
      {choices.map((choice) => {
        const selected = choice.id === value
        return (
          <label className={['aw-preset', selected ? 'is-selected' : '', disabled ? 'is-disabled' : ''].filter(Boolean).join(' ')} key={choice.id ?? 'none'}>
            <input
              checked={selected}
              className="visually-hidden"
              disabled={disabled}
              name={name}
              onChange={() => onChange(choice.id)}
              type="radio"
              value={choice.id ?? ''}
            />
            <MiniPreview shape={choice.shape} />
            <span className="aw-preset__text">
              <span className="aw-preset__name">
                <Icon name={choice.icon} size={16} />
                {choice.name}
              </span>
              <span className="aw-preset__desc">{choice.description}</span>
              {choice.tag ? <span className="aw-preset__tag">{choice.tag}</span> : null}
            </span>
            {selected ? (
              <span aria-hidden="true" className="aw-preset__check">
                <Icon name="check" size={12} />
              </span>
            ) : null}
          </label>
        )
      })}
    </div>
  )
}

const TASK_VIEWS: { part: PartId; label: (shape: Shape) => string }[] = [
  { part: 'area.board', label: () => 'Board' },
  { part: 'area.timeline', label: () => 'Timeline' },
  { part: 'area.milestones', label: (shape) => capital(shapeWord(shape, 'milestone', 'many')) },
  { part: 'area.planning', label: () => 'Planning' },
]

/** The project's sidebar as a layout gives it, with what its tasks and messages show. */
export function SidebarPreview({ projectName, shape }: { projectName: string; shape: Shape }) {
  const taskViews = shape.visible.has('area.tasks')
    ? ['List', ...TASK_VIEWS.filter((view) => shape.visible.has(view.part)).map((view) => view.label(shape))]
    : []
  const taskParts = REGISTRY.parts.filter((part) => part.group === 'task' && shape.visible.has(part.id)).map((part) => part.label)
  const talkParts = REGISTRY.parts.filter((part) => part.group === 'talk' && shape.visible.has(part.id)).map((part) => part.label)
  const hidden = REGISTRY.parts.filter((part) => !shape.visible.has(part.id)).length
  const fact = (title: string, items: string[], none: string) => (
    <div className="aw-layoutpreview__fact">
      <dt>{title}</dt>
      <dd>
        {items.length ? (
          items.map((item) => (
            <span className="aw-layoutpreview__chip" key={item}>
              {item}
            </span>
          ))
        ) : (
          <span className="aw-layoutpreview__none">{none}</span>
        )}
      </dd>
    </div>
  )
  return (
    <section aria-label="Preview" className="aw-layoutpreview">
      <h2 className="aw-layoutpreview__title">Preview</h2>
      <div aria-hidden="true" className="aw-layoutpreview__frame" data-region="sidebar">
        <span className="aw-layoutpreview__project">{projectName}</span>
        <span className="aw-layoutpreview__row is-first">
          <Icon name="home" size={16} />
          Overview
        </span>
        {topRows(shape).map((row) => (
          <span className="aw-layoutpreview__row" key={row.part}>
            <Icon name={row.icon} size={16} />
            {row.label}
          </span>
        ))}
        {shape.visible.has('area.agents') ? (
          <>
            <span className="aw-layoutpreview__head">Agents</span>
            <span className="aw-layoutpreview__row">
              <Icon name="bot" size={16} />
              Reviewer
            </span>
          </>
        ) : null}
        {shape.visible.has('area.chat') ? (
          <>
            <span className="aw-layoutpreview__head">{capital(shapeWord(shape, 'channel', 'many'))}</span>
            <span className="aw-layoutpreview__row">
              <Icon name="hash" size={16} />
              general
            </span>
            {shape.visible.has('talk.threads') ? <span className="aw-layoutpreview__row is-nested">Plans for the week</span> : null}
          </>
        ) : null}
      </div>
      <dl className="aw-layoutpreview__facts">
        {shape.visible.has('area.tasks') ? fact(`${capital(shapeWord(shape, 'task', 'many'))} as`, taskViews, 'A list') : null}
        {shape.visible.has('area.tasks') ? fact(`Each ${shapeWord(shape, 'task')} shows`, taskParts, 'Its title, status and who has it') : null}
        {shape.visible.has('area.chat') ? fact('Messages offer', talkParts, 'Writing and reading') : null}
      </dl>
      <p className="aw-layoutpreview__none">{hidden ? `${hidden} of ${REGISTRY.parts.length} parts hidden.` : 'Nothing hidden.'}</p>
    </section>
  )
}

/** One switch per part, in a box per group. */
export function PartSwitches({
  disabled = false,
  lockedOff,
  onToggle,
  visible,
}: {
  disabled?: boolean
  /** Parts that stay off here, with why: "My view" can't show what the layout hides. */
  lockedOff?: { parts: Set<string>; why: string }
  onToggle: (part: PartId, shown: boolean) => void
  visible: Set<PartId>
}) {
  return (
    <div className="aw-partgroups">
      {REGISTRY.groups.map((group) => (
        <fieldset className="aw-partgroup" key={group.id}>
          <legend className="aw-partgroup__title">{group.label}</legend>
          <p className="aw-partgroup__desc">{group.description}</p>
          {REGISTRY.parts
            .filter((part) => part.group === group.id)
            .map((part) => {
              const locked = lockedOff?.parts.has(part.id) ?? false
              return (
                <Switch
                  checked={!locked && visible.has(part.id)}
                  description={locked ? `${part.description} ${lockedOff?.why}` : part.description}
                  disabled={disabled || locked}
                  key={part.id}
                  label={part.label}
                  onChange={(shown) => onToggle(part.id, shown)}
                />
              )
            })}
        </fieldset>
      ))}
    </div>
  )
}

/** A field pair per noun: the word for one and for many. Empty keeps the preset's word. */
export function WordFields({
  base,
  disabled = false,
  onChange,
  words,
}: {
  base: Shape
  disabled?: boolean
  onChange: (noun: NounId, word: Word) => void
  words: Partial<Record<NounId, Word>>
}) {
  return (
    <div className="aw-words">
      {REGISTRY.nouns.map((noun) => {
        const own = base.words[noun.id] ?? { one: noun.one, many: noun.many }
        const word = words[noun.id] ?? own
        return (
          <div className="aw-words__row" key={noun.id}>
            <span className="aw-words__noun">{noun.label}</span>
            <TextField
              disabled={disabled}
              label="One"
              maxLength={30}
              onChange={(event) => onChange(noun.id, { ...word, one: event.target.value })}
              placeholder={own.one}
              value={word.one}
            />
            <TextField
              disabled={disabled}
              label="Many"
              maxLength={30}
              onChange={(event) => onChange(noun.id, { ...word, many: event.target.value })}
              placeholder={own.many}
              value={word.many}
            />
          </div>
        )
      })}
    </div>
  )
}

/** Says, once, that a layout is about what people see and never what they may do. */
export function VisualOnlyNote({ children }: { children?: ReactNode }) {
  return (
    <Banner className="aw-layout__note" role="note" tone="info">
      Layouts only change what people see. What anyone may do stays with their role, and a hidden part still opens from a link.
      {children ? <> {children}</> : null}
    </Banner>
  )
}

function sameLayout(left: Layout | null, right: Layout | null) {
  if (!left || !right) return left === right
  const entries = (record: object | undefined) =>
    JSON.stringify(Object.entries(record ?? {}).sort(([a], [b]) => a.localeCompare(b)))
  return left.preset === right.preset && entries(left.changes) === entries(right.changes) && entries(left.words) === entries(right.words)
}

type Draft = { preset: string | null; visible: Set<PartId>; words: Partial<Record<NounId, Word>> }

function draftFor(preset: string | null, shape: Shape): Draft {
  return { preset, visible: new Set(shape.visible), words: { ...shape.words } }
}

/**
 * Pick a preset, then change single parts and words, with the sidebar it
 * gives beside it. Changes wait for Save; "Save as preset" keeps them for
 * every project in the workspace.
 */
export function LayoutEditor({
  canEdit,
  canSavePreset,
  cardsLabel,
  fallback,
  onSave,
  onSavePreset,
  previewName,
  saved,
  stored,
}: {
  canEdit: boolean
  canSavePreset: boolean
  cardsLabel: string
  /** What applies with no layout chosen here, and its card. */
  fallback: { name: string; description: string; layout: Layout | null }
  onSave: (layout: Layout | null) => Promise<unknown>
  onSavePreset: (name: string, description: string, layout: Layout) => Promise<unknown>
  previewName: string
  saved: SavedPreset[]
  stored: Layout | null
}) {
  const fallbackShape = useMemo(() => layoutShape(fallback.layout, saved), [fallback.layout, saved])
  const initial = () => (stored ? draftFor(stored.preset, layoutShape(stored, saved)) : draftFor(null, fallbackShape))
  const [draft, setDraft] = useState<Draft>(initial)
  const [saving, setSaving] = useState(false)
  const [naming, setNaming] = useState(false)
  const choices = presetChoices(saved, { name: fallback.name, description: fallback.description, shape: fallbackShape }, draft.preset)
  // Words and switches change from the preset picked, or from what applies without one.
  const base = draft.preset ? (presetShape(draft.preset, saved) ?? everything()) : fallbackShape
  const startFrom = draft.preset ?? fallback.layout?.preset ?? 'software_team'
  const current = draft.preset === null ? null : layoutFrom(draft.preset, draft.visible, draft.words, saved)
  const dirty = !sameLayout(current, stored)
  const shape: Shape = { visible: draft.visible, words: { ...base.words, ...draft.words }, lead: base.lead }

  // Changing a part with no layout of its own starts one from what applies now.
  const toggle = (part: PartId, shown: boolean) =>
    setDraft((now) => {
      const visible = new Set(now.visible)
      if (shown) visible.add(part)
      else visible.delete(part)
      return { ...now, preset: now.preset ?? startFrom, visible }
    })
  const rename = (noun: NounId, word: Word) => setDraft((now) => ({ ...now, preset: now.preset ?? startFrom, words: { ...now.words, [noun]: word } }))
  const pick = (id: string | null) => setDraft(id ? draftFor(id, presetShape(id, saved) ?? everything()) : draftFor(null, fallbackShape))

  const save = () => {
    setSaving(true)
    onSave(current)
      .catch(() => undefined)
      .finally(() => setSaving(false))
  }

  return (
    <div className="aw-layout__editor">
      <div className="aw-layout__main">
        <section aria-labelledby="layout-presets" className="aw-pagesection">
          <div className="aw-pagesection__head">
            <div>
              <h2 className="aw-pagesection__title" id="layout-presets">
                {cardsLabel}
              </h2>
              <p className="aw-pagesection__desc">Pick the preset closest to how it’s used. You can change single parts and words below.</p>
            </div>
          </div>
          <PresetCards choices={choices} disabled={!canEdit} label={cardsLabel} onChange={pick} value={draft.preset} />
        </section>
        <section aria-labelledby="layout-parts" className="aw-pagesection">
          <div className="aw-pagesection__head">
            <div>
              <h2 className="aw-pagesection__title" id="layout-parts">
                Parts
              </h2>
              <p className="aw-pagesection__desc">Turn single parts on or off. Anything already in a hidden part stays as it is.</p>
            </div>
          </div>
          <PartSwitches disabled={!canEdit} onToggle={toggle} visible={draft.visible} />
        </section>
        <section aria-labelledby="layout-words" className="aw-pagesection">
          <div className="aw-pagesection__head">
            <div>
              <h2 className="aw-pagesection__title" id="layout-words">
                Words
              </h2>
              <p className="aw-pagesection__desc">Call things what the people here call them, such as to-dos instead of tasks.</p>
            </div>
          </div>
          <WordFields base={base} disabled={!canEdit} onChange={rename} words={draft.words} />
        </section>
        {canEdit ? (
          <div className="aw-layout__actions">
            <span aria-live="polite" className="aw-layout__status">
              {dirty ? 'Unsaved changes' : 'Saved'}
            </span>
            {canSavePreset ? (
              <Button icon="bookmark" onClick={() => setNaming(true)} variant="ghost">
                Save as preset…
              </Button>
            ) : null}
            <Button disabled={!dirty || saving} onClick={() => setDraft(initial())} variant="secondary">
              Discard
            </Button>
            <Button busy={saving} disabled={!dirty} onClick={save} variant="primary">
              Save layout
            </Button>
          </div>
        ) : null}
      </div>
      <aside className="aw-layout__aside">
        <SidebarPreview projectName={previewName} shape={shape} />
      </aside>
      {naming ? (
        <SavePresetDialog
          onClose={() => setNaming(false)}
          onSave={(name, description) =>
            onSavePreset(name, description, current ?? layoutFrom(startFrom, draft.visible, draft.words, saved)).then(() => setNaming(false))
          }
        />
      ) : null}
    </div>
  )
}

/** Name a layout to use again in any project of the workspace. */
export function SavePresetDialog({
  initial,
  onClose,
  onSave,
  title = 'Save as preset',
}: {
  initial?: { name: string; description: string }
  onClose: () => void
  onSave: (name: string, description: string) => Promise<unknown>
  title?: string
}) {
  const [name, setName] = useState(initial?.name ?? '')
  const [description, setDescription] = useState(initial?.description ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim()) return
    setBusy(true)
    setError(null)
    onSave(name.trim(), description.trim())
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : 'The preset wasn’t saved.'))
      .finally(() => setBusy(false))
  }
  return (
    <Dialog
      description="Saved presets show up for every project in this workspace, next to the built-in ones."
      dirty={Boolean(name.trim() || description.trim())}
      footer={
        <>
          <Button onClick={onClose} variant="secondary">
            Cancel
          </Button>
          <Button busy={busy} disabled={!name.trim()} type="submit" variant="primary">
            Save preset
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title={title}
    >
      <TextField data-autofocus label="Name" maxLength={60} onChange={(event) => setName(event.target.value)} placeholder="e.g. Family chores" required value={name} />
      <TextArea
        label="Description"
        maxLength={200}
        onChange={(event) => setDescription(event.target.value)}
        placeholder="What it’s for, in a line"
        rows={2}
        value={description}
      />
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </Dialog>
  )
}
