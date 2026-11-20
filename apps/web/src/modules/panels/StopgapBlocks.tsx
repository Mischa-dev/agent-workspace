// A stopgap for W1.25's PanelBlocks (Lane C2), with the same props, so
// PanelHost can draw a connected module's panel until the real renderer
// lands; then `PanelHost` imports `pieces/blocks/PanelBlocks` instead and
// this file goes. It draws every aw-panel/1 block plainly with kit parts.
// Text is never HTML.

import './panels.css'

import { useState, type FormEvent, type ReactNode } from 'react'

import { Button, Checkbox, ConfirmDialog, DetailList, SelectField, StatusChip, TextArea, TextField, type ChipTone } from '../../ui'
import { formatDateTime } from '../../ui/time'

export type PanelAction = { id: string; values?: Record<string, unknown> }
export type PanelBlock = { type: string; [key: string]: unknown }

export type PanelBlocksProps = {
  blocks: PanelBlock[]
  /** A press or a form's submit. A rejection shows as the form's error. */
  onAction: (action: PanelAction) => void | Promise<unknown>
  /** The module's messages for the form's fields, by field id. */
  errors?: Record<string, string>
  label?: string
}

const record = (value: unknown): Record<string, unknown> =>
  value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {}
const list = (value: unknown): unknown[] => (Array.isArray(value) ? value : [])
const text = (value: unknown): string => (typeof value === 'string' ? value : typeof value === 'number' ? String(value) : '')

/** An https address, or a path in this app. */
function safeHref(href: string): string | null {
  if (href.startsWith('/')) return href.startsWith('//') || href.startsWith('/\\') ? null : href
  try {
    return new URL(href).protocol === 'https:' ? href : null
  } catch {
    return null
  }
}

/** A press outside a form: its failure shows above the panel, not here. */
function fire(onAction: PanelBlocksProps['onAction'], action: PanelAction) {
  void Promise.resolve(onAction(action)).catch(() => undefined)
}

const TONES: Record<string, ChipTone> = {
  success: 'success',
  warning: 'warning',
  danger: 'danger',
  agent: 'agent',
}

function badge(value: unknown) {
  const shown = typeof value === 'string' ? { text: value, tone: undefined } : record(value)
  return <StatusChip tone={TONES[text(shown.tone)] ?? 'muted'}>{text(shown.text)}</StatusChip>
}

function shownValue(value: unknown, kind: string): ReactNode {
  if (value === null || value === undefined) return '–'
  switch (kind) {
    case 'badge':
      return badge(value)
    case 'time':
    case 'date':
      return typeof value === 'string' && !Number.isNaN(Date.parse(value)) ? formatDateTime(value) : text(value)
    case 'link': {
      const link = typeof value === 'string' ? { href: value, text: value } : record(value)
      const href = safeHref(text(link.href))
      const label = text(link.text) || text(link.href)
      return href ? (
        <a href={href} rel="noreferrer" target={href.startsWith('/') ? undefined : '_blank'}>
          {label}
        </a>
      ) : (
        label
      )
    }
    case 'thing':
    case 'person': {
      const reference = record(value)
      return text(reference.label) || text(reference.id)
    }
    default:
      return typeof value === 'boolean' ? (value ? 'Yes' : 'No') : text(value)
  }
}

function Buttons({ block, onAction }: { block: PanelBlock; onAction: PanelBlocksProps['onAction'] }) {
  const [asking, setAsking] = useState<Record<string, unknown> | null>(null)
  const press = (item: Record<string, unknown>) => {
    if (item.confirm) setAsking(item)
    else fire(onAction, record(item.action) as PanelAction)
  }
  const confirm = record(asking?.confirm)
  return (
    <div className="aw-panel-buttons" data-part="panel-buttons">
      {list(block.items).map((raw, index) => {
        const item = record(raw)
        const style = text(item.style)
        return (
          <Button
            key={`${text(record(item.action).id)}-${index}`}
            onClick={() => press(item)}
            size="sm"
            variant={style === 'primary' ? 'primary' : style === 'danger' ? 'danger' : 'secondary'}
          >
            {text(item.label)}
          </Button>
        )
      })}
      <ConfirmDialog
        confirmLabel={text(asking?.label)}
        onClose={() => setAsking(null)}
        onConfirm={() => {
          const action = record(asking?.action) as PanelAction
          setAsking(null)
          fire(onAction, action)
        }}
        open={asking !== null}
        title={text(confirm.title)}
        tone={text(asking?.style) === 'danger' ? 'danger' : 'primary'}
      >
        {text(confirm.body) || undefined}
      </ConfirmDialog>
    </div>
  )
}

function Form({ block, errors, onAction }: { block: PanelBlock; errors: Record<string, string>; onAction: PanelBlocksProps['onAction'] }) {
  const schema = record(block.schema)
  const properties = record(schema.properties)
  const required = new Set(list(schema.required).map(text))
  const ui = record(block.ui)
  const widgets = record(ui.widgets)
  const order = [...list(ui.order).map(text).filter((name) => name in properties), ...Object.keys(properties)].filter(
    (name, index, names) => names.indexOf(name) === index,
  )
  const [values, setValues] = useState<Record<string, unknown>>(() => {
    const start: Record<string, unknown> = {}
    for (const name of order) {
      const field = record(properties[name])
      start[name] = record(block.values)[name] ?? field.default ?? (field.type === 'boolean' ? false : field.type === 'array' ? [] : '')
    }
    return start
  })
  const [busy, setBusy] = useState(false)
  const [failed, setFailed] = useState<string | null>(null)
  const submit = record(block.submit)
  const cancel = block.cancel ? record(block.cancel) : null
  const send = async (event: FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setFailed(null)
    try {
      await onAction({ id: text(record(submit.action).id), values })
    } catch (error) {
      setFailed(error instanceof Error ? error.message : 'That didn’t go through.')
    } finally {
      setBusy(false)
    }
  }
  const set = (name: string, value: unknown) => setValues((current) => ({ ...current, [name]: value }))
  return (
    <form className="aw-panel-form" data-part="panel-form" onSubmit={(event) => void send(event)}>
      {order.map((name) => {
        const field = record(properties[name])
        const label = text(field.title) || name
        const hint = text(field.description) || undefined
        const error = errors[name]
        const choices = list(field.enum ?? record(field.items).enum).map(text)
        const widget = text(widgets[name])
        if (field.type === 'boolean') {
          return <Checkbox checked={values[name] === true} description={error ?? hint} key={name} label={label} onChange={(checked) => set(name, checked)} />
        }
        if (field.type === 'array') {
          const picked = new Set(list(values[name]).map(text))
          return (
            <fieldset className="aw-panel-form__choices" key={name}>
              <legend>{label}</legend>
              {choices.map((choice) => (
                <Checkbox
                  checked={picked.has(choice)}
                  key={choice}
                  label={choice}
                  onChange={(checked) => set(name, checked ? [...picked, choice] : [...picked].filter((item) => item !== choice))}
                />
              ))}
              {error ? <p className="aw-field__note" role="alert">{error}</p> : null}
            </fieldset>
          )
        }
        if (choices.length > 0 || widget === 'select' || widget === 'radio') {
          return (
            <SelectField error={error} hint={hint} key={name} label={label} onChange={(event) => set(name, event.target.value)} required={required.has(name)} value={text(values[name])}>
              {required.has(name) ? null : <option value="">–</option>}
              {choices.map((choice) => (
                <option key={choice} value={choice}>
                  {choice}
                </option>
              ))}
            </SelectField>
          )
        }
        if (widget === 'textarea') {
          return <TextArea error={error} hint={hint} key={name} label={label} onChange={(event) => set(name, event.target.value)} required={required.has(name)} value={text(values[name])} />
        }
        const numeric = field.type === 'number' || field.type === 'integer'
        return (
          <TextField
            error={error}
            hint={hint}
            key={name}
            label={label}
            onChange={(event) => set(name, numeric ? (event.target.value === '' ? '' : Number(event.target.value)) : event.target.value)}
            required={required.has(name)}
            type={numeric ? 'number' : 'text'}
            value={text(values[name])}
          />
        )
      })}
      {failed ? (
        <p className="aw-field__note" role="alert">
          {failed}
        </p>
      ) : null}
      <div className="aw-panel-buttons">
        <Button busy={busy} type="submit" variant="primary">
          {text(submit.label) || 'Send'}
        </Button>
        {cancel ? (
          <Button onClick={() => cancel.action && fire(onAction, record(cancel.action) as PanelAction)} variant="ghost">
            {text(cancel.label) || 'Cancel'}
          </Button>
        ) : null}
      </div>
    </form>
  )
}

function Block({ block, errors, onAction }: { block: PanelBlock; errors: Record<string, string>; onAction: PanelBlocksProps['onAction'] }) {
  switch (block.type) {
    case 'text': {
      const style = text(block.style)
      if (style === 'heading') return <h3 className="aw-panel-text aw-panel-text--heading">{text(block.text)}</h3>
      return <p className={`aw-panel-text aw-panel-text--${style || 'body'}`}>{text(block.text)}</p>
    }
    case 'status': {
      const state = text(block.state)
      const tone: ChipTone = state === 'ok' ? 'success' : state === 'error' ? 'danger' : state === 'working' ? 'agent' : state === 'waiting' ? 'warning' : 'muted'
      const progress = typeof block.progress === 'number' ? block.progress : null
      return (
        <div className="aw-panel-status" data-part="panel-status">
          <StatusChip dots tone={tone}>
            {text(block.label)}
          </StatusChip>
          {block.detail ? <p className="aw-panel-text aw-panel-text--muted">{text(block.detail)}</p> : null}
          {progress !== null ? <progress aria-label={text(block.label)} max={1} value={progress} /> : null}
        </div>
      )
    }
    case 'fields':
      return (
        <DetailList
          items={list(block.items).map((raw) => {
            const item = record(raw)
            return { label: text(item.label), value: shownValue(item.value, text(item.kind) || 'text') }
          })}
        />
      )
    case 'list':
      return list(block.items).length === 0 ? (
        <p className="aw-panel-text aw-panel-text--muted">{text(block.empty) || 'Nothing here yet.'}</p>
      ) : (
        <ul className="aw-panel-list" data-part="panel-list">
          {list(block.items).map((raw) => {
            const item = record(raw)
            const action = item.action ? (record(item.action) as PanelAction) : null
            const body = (
              <>
                <span className="aw-panel-list__title">{text(item.title)}</span>
                {item.subtitle ? <span className="aw-panel-text--muted">{text(item.subtitle)}</span> : null}
                {item.status ? badge(item.status) : null}
                {item.meta ? <span className="aw-panel-text--muted">{text(item.meta)}</span> : null}
              </>
            )
            return (
              <li key={text(item.id)}>
                {action ? (
                  <button className="aw-panel-list__row" onClick={() => fire(onAction, action)} type="button">
                    {body}
                  </button>
                ) : (
                  <div className="aw-panel-list__row">{body}</div>
                )}
              </li>
            )
          })}
        </ul>
      )
    case 'table': {
      const columns = list(block.columns).map(record)
      const rowAction = block.rowAction ? record(block.rowAction) : null
      return (
        <div className="aw-panel-table" data-part="panel-table">
          <table>
            <thead>
              <tr>
                {columns.map((column) => (
                  <th key={text(column.id)} scope="col">
                    {text(column.label)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {list(block.rows).map((raw) => {
                const row = record(raw)
                const cells = record(row.cells)
                return (
                  <tr key={text(row.id)}>
                    {columns.map((column, index) => {
                      const value = shownValue(cells[text(column.id)], text(column.kind) || 'text')
                      return (
                        <td data-label={text(column.label)} key={text(column.id)}>
                          {index === 0 && rowAction ? (
                            <button
                              className="aw-panel-table__open"
                              onClick={() => fire(onAction, { id: text(rowAction.id), values: { ...record(rowAction.values), row: text(row.id) } })}
                              type="button"
                            >
                              {value}
                            </button>
                          ) : (
                            value
                          )}
                        </td>
                      )
                    })}
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )
    }
    case 'chart': {
      const unit = text(block.unit)
      if (block.kind === 'number') {
        return (
          <p className="aw-panel-number" data-part="panel-number">
            {text(block.value)}
            {unit ? <span className="aw-panel-text--muted"> {unit}</span> : null}
          </p>
        )
      }
      const x = list(block.x).map(text)
      const series = list(block.series).map(record)
      // The stopgap draws a chart as its numbers; W1.25 draws it.
      return (
        <div className="aw-panel-table" data-part="panel-chart">
          <table>
            <thead>
              <tr>
                <th scope="col">
                  <span className="visually-hidden">Series</span>
                </th>
                {x.map((label) => (
                  <th key={label} scope="col">
                    {label}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {series.map((one) => (
                <tr key={text(one.name)}>
                  <th scope="row">{text(one.name)}</th>
                  {list(one.values).map((value, index) => (
                    <td data-label={x[index]} key={x[index] ?? index}>
                      {text(value)}
                      {unit && value !== null ? ` ${unit}` : ''}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )
    }
    case 'buttons':
      return <Buttons block={block} onAction={onAction} />
    case 'form':
      return <Form block={block} errors={errors} onAction={onAction} />
    case 'section':
      return (
        <section className="aw-panel-section" data-part="panel-section">
          {block.title ? <h3 className="aw-panel-text aw-panel-text--heading">{text(block.title)}</h3> : null}
          <PanelBlocks blocks={list(block.blocks) as PanelBlock[]} errors={errors} onAction={onAction} />
        </section>
      )
    default:
      return <p className="aw-panel-text aw-panel-text--muted">Part of this panel needs a newer Workspace.</p>
  }
}

export function PanelBlocks({ blocks, errors = {}, label, onAction }: PanelBlocksProps) {
  return (
    <div aria-label={label} className="aw-panel-blocks" data-part="panel-blocks" role={label ? 'group' : undefined}>
      {blocks.map((block, index) => (
        <Block block={block} errors={errors} key={index} onAction={onAction} />
      ))}
    </div>
  )
}
