import { useId, useState, type FormEvent, type ReactNode } from 'react'
import { Link } from 'react-router-dom'

import { Button, Checkbox, DetailList, Menu, SelectField, StatusChip, TextField } from '../../ui'
import type { FieldDecl, FieldValue, Piece, Thing, ThingDecl } from '../contract'
import { fieldOf, fieldText, rawValue, words } from './fields'
import './pieces.css'

// Pieces the app draws (AWD-75, principle 5 of the build-out plan). A module
// never draws its own layout: its manifest names a piece (a list, a table, a
// board, a detail view, fields, a form, a card or actions) of one kind of
// thing with some fields, and these draw it, for every width. Each piece is
// its own container: a table turns into cards when it's narrow, a board
// scrolls sideways inside itself, and actions fold into a menu. Fix a piece
// here and every module gets the fix.

export type PieceProps = {
  piece: Piece
  /** The kind of thing it shows, as its module declares it. */
  decl: ThingDecl
  things: Thing[]
  /** What the piece is called, for screen readers and headings. */
  label: string
  /** An action chosen on one thing, such as `open`. */
  onAction?: (action: string, thing: Thing | null) => void
  /** A form's values when it's sent. */
  onSubmit?: (values: Record<string, FieldValue>) => void
}

/** Draws any piece. */
export function PieceView(props: PieceProps) {
  const Draw = DRAW[props.piece.type]
  return (
    <div className={`aw-piece aw-piece--${props.piece.type}`} data-piece={props.piece.type}>
      <Draw {...props} />
    </div>
  )
}

const DRAW: Record<Piece['type'], (props: PieceProps) => ReactNode> = {
  list: ListPiece,
  table: TablePiece,
  board: BoardPiece,
  detail: DetailPiece,
  fields: FieldsPiece,
  form: FormPiece,
  card: CardPiece,
  actions: ActionsPiece,
}

function FieldValueView({ decl, id, thing }: { decl: ThingDecl; id: string; thing: Thing }) {
  const field = fieldOf(decl, id)
  const text = fieldText(field, rawValue(thing, id))
  if (text === null) return <span className="aw-piece__none">None</span>
  if (field.type === 'status') return <StatusChip>{text}</StatusChip>
  return <span className="aw-piece__value">{text}</span>
}

function ThingTitle({ thing }: { thing: Thing }) {
  return thing.href.startsWith('/') ? (
    <Link className="aw-piece__title" to={thing.href}>
      {thing.title}
    </Link>
  ) : (
    <a className="aw-piece__title" href={thing.href} rel="noreferrer" target="_blank">
      {thing.title}
    </a>
  )
}

/** The piece's fields other than the title, which every piece shows anyway. */
const shownFields = (piece: Piece) => (piece.fields ?? ['status', 'owner']).filter((field) => field !== 'title')

function Meta({ decl, piece, thing }: { decl: ThingDecl; piece: Piece; thing: Thing }) {
  return (
    <dl className="aw-piece__meta">
      {shownFields(piece).map((id) => (
        <div className="aw-piece__meta-item" key={id}>
          <dt className="visually-hidden">{fieldOf(decl, id).label}</dt>
          <dd>
            <FieldValueView decl={decl} id={id} thing={thing} />
          </dd>
        </div>
      ))}
    </dl>
  )
}

function groups(piece: Piece, decl: ThingDecl, things: Thing[]) {
  const by = piece.groupBy
  if (!by) return [{ key: '', title: '', things }]
  const field = fieldOf(decl, by)
  const found = new Map<string, Thing[]>()
  for (const thing of things) {
    const key = fieldText(field, rawValue(thing, by)) ?? 'None'
    found.set(key, [...(found.get(key) ?? []), thing])
  }
  return [...found].map(([key, grouped]) => ({ key, title: key, things: grouped }))
}

function Empty({ decl }: { decl: ThingDecl }) {
  return <p className="aw-piece__empty">No {decl.many.toLowerCase()} yet.</p>
}

// ---------------------------------------------------------------------------
// The pieces

function ListPiece({ decl, label, piece, things }: PieceProps) {
  if (!things.length) return <Empty decl={decl} />
  return (
    <section aria-label={label}>
      {groups(piece, decl, things).map((group) => (
        <div className="aw-piece__group" key={group.key}>
          {group.title ? <h3 className="aw-piece__group-title">{group.title}</h3> : null}
          <ul className="aw-piece__rows">
            {group.things.map((thing) => (
              <li className="aw-piece__row" key={thing.id}>
                <ThingTitle thing={thing} />
                <Meta decl={decl} piece={piece} thing={thing} />
              </li>
            ))}
          </ul>
        </div>
      ))}
    </section>
  )
}

function TablePiece({ decl, label, piece, things }: PieceProps) {
  if (!things.length) return <Empty decl={decl} />
  const columns = ['title', ...shownFields(piece)]
  return (
    <table aria-label={label} className="aw-piece__table">
      <thead>
        <tr>
          {columns.map((id) => (
            <th key={id} scope="col">
              {fieldOf(decl, id).label}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {things.map((thing) => (
          <tr key={thing.id}>
            {columns.map((id) => (
              // On a narrow screen each row is a card, and each cell says what it is.
              <td data-label={fieldOf(decl, id).label} key={id}>
                {id === 'title' ? <ThingTitle thing={thing} /> : <FieldValueView decl={decl} id={id} thing={thing} />}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  )
}

function BoardPiece({ decl, label, piece, things }: PieceProps) {
  if (!things.length) return <Empty decl={decl} />
  const columns = groups({ ...piece, groupBy: piece.groupBy ?? 'status' }, decl, things)
  return (
    // Columns scroll sideways inside the board, never the page.
    <section aria-label={label} className="aw-piece__board" data-scroll-x="" tabIndex={0}>
      {columns.map((column) => (
        <div className="aw-piece__column" key={column.key}>
          <h3 className="aw-piece__group-title">
            {column.title} <span className="aw-piece__count">{column.things.length}</span>
          </h3>
          <ul className="aw-piece__cards">
            {column.things.map((thing) => (
              <li className="aw-piece__card" key={thing.id}>
                <ThingTitle thing={thing} />
                <Meta decl={decl} piece={{ ...piece, fields: shownFields(piece).filter((id) => id !== column.key) }} thing={thing} />
              </li>
            ))}
          </ul>
        </div>
      ))}
    </section>
  )
}

function DetailPiece(props: PieceProps) {
  const { decl, piece, things } = props
  const thing = things[0]
  if (!thing) return <Empty decl={decl} />
  const fields = shownFields(piece).filter((id) => id !== 'status')
  return (
    <article aria-label={props.label} className="aw-piece__detail">
      <header className="aw-piece__detail-head">
        <h3 className="aw-piece__detail-title">{thing.title}</h3>
        {thing.status ? <StatusChip>{thing.status}</StatusChip> : null}
      </header>
      <DetailList items={fields.map((id) => ({ label: fieldOf(decl, id).label, value: <FieldValueView decl={decl} id={id} thing={thing} /> }))} />
      {piece.actions?.length ? <ActionsPiece {...props} /> : null}
    </article>
  )
}

function FieldsPiece({ decl, piece, things }: PieceProps) {
  const thing = things[0]
  if (!thing) return <Empty decl={decl} />
  return <DetailList items={shownFields(piece).map((id) => ({ label: fieldOf(decl, id).label, value: <FieldValueView decl={decl} id={id} thing={thing} /> }))} />
}

function FormPiece({ decl, label, onSubmit, piece }: PieceProps) {
  const fields = (piece.fields ?? ['title']).map((id) => fieldOf(decl, id))
  const [values, setValues] = useState<Record<string, FieldValue>>({})
  const set = (id: string, value: FieldValue) => setValues((current) => ({ ...current, [id]: value }))
  const submit = (event: FormEvent) => {
    event.preventDefault()
    onSubmit?.(values)
  }
  return (
    <form aria-label={label} className="aw-piece__form" onSubmit={submit}>
      {fields.map((field) => (
        <FormField field={field} key={field.id} onChange={(value) => set(field.id, value)} value={values[field.id] ?? null} />
      ))}
      <div className="aw-piece__form-actions">
        <Button type="submit" variant="primary">
          Add {decl.one.toLowerCase()}
        </Button>
      </div>
    </form>
  )
}

function FormField({ field, onChange, value }: { field: FieldDecl; onChange: (value: FieldValue) => void; value: FieldValue }) {
  if (field.type === 'checkbox') return <Checkbox checked={value === true} label={field.label} onChange={onChange} />
  if (field.type === 'multiple_choice' && field.options?.length) {
    const chosen = Array.isArray(value) ? value : []
    return (
      <fieldset className="aw-piece__choices">
        <legend className="aw-piece__group-title">{field.label}</legend>
        {field.options.map((option) => (
          <Checkbox
            checked={chosen.includes(option)}
            key={option}
            label={words(option)}
            onChange={(on) => onChange(on ? [...chosen, option] : chosen.filter((entry) => entry !== option))}
          />
        ))}
      </fieldset>
    )
  }
  if (field.type === 'single_choice' && field.options?.length) {
    return (
      <SelectField label={field.label} onChange={(event) => onChange(event.target.value || null)} value={typeof value === 'string' ? value : ''}>
        <option value="">None</option>
        {field.options.map((option) => (
          <option key={option} value={option}>
            {words(option)}
          </option>
        ))}
      </SelectField>
    )
  }
  const type = field.type === 'number' ? 'number' : field.type === 'date' ? 'date' : field.type === 'url' ? 'url' : 'text'
  return (
    <TextField
      label={field.label}
      onChange={(event) => onChange(event.target.value === '' ? null : type === 'number' ? Number(event.target.value) : event.target.value)}
      type={type}
      value={value === null || Array.isArray(value) || typeof value === 'boolean' ? '' : String(value)}
    />
  )
}

function CardPiece({ decl, piece, things }: PieceProps) {
  const thing = things[0]
  if (!thing) return <Empty decl={decl} />
  return (
    <article className="aw-piece__card aw-piece__card--alone">
      <p className="aw-piece__kind">{decl.one}</p>
      <ThingTitle thing={thing} />
      <Meta decl={decl} piece={piece} thing={thing} />
    </article>
  )
}

const ACTION_WORDS: Record<string, string> = { open: 'Open', edit: 'Edit', archive: 'Archive', link: 'Link' }

function ActionsPiece({ decl, label, onAction, piece, things }: PieceProps) {
  const id = useId()
  const actions = piece.actions?.length ? piece.actions : ['open']
  const thing = things[0] ?? null
  const name = (action: string) => ACTION_WORDS[action] ?? words(action)
  return (
    <div aria-label={label} className="aw-piece__actions" id={id} role="group">
      {/* Wide: buttons. Narrow: one menu, so nothing runs off the edge. */}
      <div className="aw-piece__buttons">
        {actions.map((action) => (
          <Button key={action} onClick={() => onAction?.(action, thing)} size="sm">
            {name(action)}
          </Button>
        ))}
      </div>
      <div className="aw-piece__menu">
        <Menu
          items={actions.map((action) => ({ label: name(action), onSelect: () => onAction?.(action, thing) }))}
          label={`${decl.one} actions`}
          trigger={<Button size="sm">Actions</Button>}
        />
      </div>
    </div>
  )
}
