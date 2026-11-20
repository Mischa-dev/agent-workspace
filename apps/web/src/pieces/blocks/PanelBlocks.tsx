import { lazy, Suspense, useId, useState, type ReactNode } from 'react'
import { Link } from 'react-router-dom'

import {
  Banner,
  Button,
  ConfirmDialog,
  DetailList,
  IconButton,
  Menu,
  StatusChip,
  formatFullDate,
  formatRelative,
  type MenuEntry,
} from '../../ui'
import { FormPiece } from '../FormPiece'
import { ListPiece, ListPieceTitle } from '../ListPiece'
import { ThingChip } from '../ThingChip'
import {
  formFields,
  formValues,
  isKnownBlock,
  safeHref,
  textRuns,
  type ButtonsBlock,
  type ChartBlock,
  type FieldKind,
  type FieldsBlock,
  type FormBlock,
  type KnownBlock,
  type ListBlock,
  type PanelAction,
  type PanelBlock,
  type PanelStatus,
  type PanelTone,
  type StatusBlock,
  type TableBlock,
  type TextBlock,
} from './format'
import '../pieces.css'
import './blocks.css'

// The panel block renderer (W1.25): draws an `aw-panel/1` document's blocks
// with the app's own pieces, wide and narrow. It never fetches: actions go
// to `onAction`, which M1.14's `PanelHost` sends to the module and answers
// with the next panel. A block it doesn't know is a quiet note, never a
// failure, so a module on a newer format still shows what it can.

const ChartFigure = lazy(() => import('../../collections/CollectionChartView').then((module) => ({ default: module.ChartFigure })))

export type PanelBlocksProps = {
  blocks: readonly PanelBlock[]
  /** A button, a row or a form asked for something. A form shows a rejection as its error. */
  onAction: (action: PanelAction) => Promise<unknown> | unknown
  /** Field errors the module answered a form with, by field id. */
  errors?: Record<string, string>
  /** What the panel is called, for screen readers. */
  label?: string
}

export function PanelBlocks({ blocks, errors, label, onAction }: PanelBlocksProps) {
  return (
    <div aria-label={label} className="aw-piece aw-blocks" data-piece="panel" role={label ? 'region' : undefined}>
      {blocks.map((block, index) => (
        <Block block={block} depth={0} errors={errors} key={index} onAction={onAction} />
      ))}
    </div>
  )
}

type DrawProps = { depth: number; errors?: Record<string, string>; onAction: PanelBlocksProps['onAction'] }

function Block({ block, ...props }: DrawProps & { block: PanelBlock }) {
  if (!isKnownBlock(block)) return <Unknown />
  return (
    <div className={`aw-block aw-block--${block.type}`} data-part={`panel-${block.type}`}>
      <KnownBlockView block={block} {...props} />
    </div>
  )
}

function KnownBlockView({ block, depth, errors, onAction }: DrawProps & { block: KnownBlock }) {
  switch (block.type) {
    case 'text':
      return <TextView block={block} />
    case 'fields':
      return <FieldsView block={block} />
    case 'form':
      return <FormView block={block} errors={errors} onAction={onAction} />
    case 'list':
      return <ListView block={block} onAction={onAction} />
    case 'table':
      return <TableView block={block} onAction={onAction} />
    case 'buttons':
      return <ButtonsView block={block} onAction={onAction} />
    case 'status':
      return <StatusView block={block} />
    case 'chart':
      return <ChartView block={block} />
    case 'section':
      return (
        <section className="aw-block__section">
          {block.title ? <h3 className="aw-block__heading">{block.title}</h3> : null}
          {/* One level of sections; a section inside a section draws its blocks flat. */}
          {block.blocks.map((inner, index) =>
            inner.type === 'section' && depth > 0 ? null : <Block block={inner} depth={depth + 1} errors={errors} key={index} onAction={onAction} />,
          )}
        </section>
      )
  }
}

function Unknown() {
  return (
    <p className="aw-block aw-block--unknown" data-part="panel-unknown">
      Part of this panel needs a newer Workspace.
    </p>
  )
}

// ---------------------------------------------------------------------------
// Text and values

/** Panel text: bold, italic, code and safe links; never HTML. */
export function PanelText({ text }: { text: string }) {
  return (
    <>
      {textRuns(text).map((run, index) => {
        switch (run.kind) {
          case 'bold':
            return <strong key={index}>{run.text}</strong>
          case 'italic':
            return <em key={index}>{run.text}</em>
          case 'code':
            return <code key={index}>{run.text}</code>
          case 'link':
            return <PanelLink href={run.href} key={index}>{run.text}</PanelLink>
          default:
            return <span key={index}>{run.text}</span>
        }
      })}
    </>
  )
}

function PanelLink({ children, href }: { children: ReactNode; href: string }) {
  if (!safeHref(href)) return <>{children}</>
  return href.startsWith('/') ? (
    <Link to={href}>{children}</Link>
  ) : (
    <a href={href} rel="noreferrer" target="_blank">
      {children}
    </a>
  )
}

function TextView({ block }: { block: TextBlock }) {
  const style = block.style ?? 'body'
  if (style === 'heading') return <h3 className="aw-block__heading">{block.text}</h3>
  if (style === 'callout') {
    return (
      <Banner tone={block.tone === 'danger' ? 'danger' : block.tone === 'warning' ? 'warning' : block.tone === 'success' ? 'success' : 'info'}>
        <PanelText text={block.text} />
      </Banner>
    )
  }
  return (
    <p className={style === 'muted' ? 'aw-block__text is-muted' : 'aw-block__text'}>
      <PanelText text={block.text} />
    </p>
  )
}

function chip(status: PanelStatus): { text: string; tone: PanelTone } {
  return typeof status === 'string' ? { text: status, tone: 'muted' } : { text: status.text, tone: status.tone ?? 'muted' }
}

/** One value as its kind reads: a time as "5 min ago", a thing as its chip, a link where it points. */
function Value({ kind = 'text', value }: { kind?: FieldKind; value: unknown }) {
  if (value === null || value === undefined || value === '') return <span className="aw-piece__none">None</span>
  if (kind === 'badge' && (typeof value === 'string' || (typeof value === 'object' && 'text' in (value as object)))) {
    const { text, tone } = chip(value as PanelStatus)
    return <StatusChip tone={tone}>{text}</StatusChip>
  }
  if ((kind === 'time' || kind === 'date') && typeof value === 'string' && !Number.isNaN(Date.parse(value))) {
    return (
      <time dateTime={value} title={formatFullDate(value)}>
        {kind === 'time' ? formatRelative(value) : new Date(value).toLocaleDateString(undefined, { dateStyle: 'medium' })}
      </time>
    )
  }
  if (kind === 'number' && typeof value === 'number') return <span className="aw-block__number">{value.toLocaleString()}</span>
  if (kind === 'link' && typeof value === 'object' && value && 'href' in value) {
    const link = value as { href: string; label?: string }
    return <PanelLink href={link.href}>{link.label ?? link.href}</PanelLink>
  }
  if (kind === 'thing' && typeof value === 'object' && value && 'kind' in value) {
    const thing = value as { kind: string; id: string; label?: string; href?: string }
    return <ThingChip href={thing.href && safeHref(thing.href) ? thing.href : undefined} kind={thing.kind} label={thing.label ?? thing.kind} />
  }
  if (kind === 'person' && typeof value === 'object' && value && 'name' in value) return <span>{(value as { name: string }).name}</span>
  return <span>{typeof value === 'object' ? JSON.stringify(value) : String(value)}</span>
}

function FieldsView({ block }: { block: FieldsBlock }) {
  return <DetailList items={block.items.map((item) => ({ label: item.label, value: <Value kind={item.kind} value={item.value} /> }))} />
}

// ---------------------------------------------------------------------------
// Forms, lists, tables and buttons: each asks through `onAction`.

function FormView({ block, errors, onAction }: { block: FormBlock; errors?: Record<string, string>; onAction: PanelBlocksProps['onAction'] }) {
  const fields = formFields(block)
  const initial = Object.fromEntries(
    Object.entries({ ...defaults(block), ...(block.values ?? {}) }).filter(
      (entry): entry is [string, string | string[] | boolean] =>
        typeof entry[1] === 'string' || typeof entry[1] === 'boolean' || Array.isArray(entry[1]) || typeof entry[1] === 'number',
    ).map(([id, value]) => [id, typeof value === 'number' ? String(value) : value]),
  )
  const messages = Object.entries(errors ?? {}).filter(([id]) => fields.some((field) => field.id === id))
  return (
    <>
      {messages.length ? (
        <Banner tone="danger">
          {messages.map(([id, message]) => (
            <span className="aw-block__error" key={id}>
              {fields.find((field) => field.id === id)?.label}: {message}
            </span>
          ))}
        </Banner>
      ) : null}
      <FormPiece
        fields={fields}
        initialValues={initial}
        key={block.id}
        label={block.submit.label}
        onSubmit={(values) => onAction({ id: block.submit.action.id, values: formValues(block, values) })}
        submitLabel={block.submit.label}
      />
      {block.cancel ? (
        <Button className="aw-block__cancel" onClick={() => void onAction({ id: block.cancel!.action.id })} variant="ghost">
          {block.cancel.label}
        </Button>
      ) : null}
    </>
  )
}

function defaults(block: FormBlock) {
  return Object.fromEntries(
    Object.entries(block.schema.properties ?? {})
      .filter(([, property]) => property.default !== undefined)
      .map(([id, property]) => [id, property.default]),
  )
}

function ListView({ block, onAction }: { block: ListBlock; onAction: PanelBlocksProps['onAction'] }) {
  return (
    <>
      <ListPiece
        aside={(item) => {
          if (!item.status) return null
          const { text, tone } = chip(item.status)
          return <StatusChip tone={tone}>{text}</StatusChip>
        }}
        empty={<p className="aw-block__text is-muted">{block.empty ?? 'Nothing here yet.'}</p>}
        itemKey={(item) => item.id}
        items={block.items}
        meta={(item) => [item.subtitle, item.meta].filter(Boolean).join(' · ') || null}
        title={(item) =>
          item.action ? (
            <button className="aw-block__rowbutton" onClick={() => void onAction({ id: item.action!.id, values: { row: item.id } })} type="button">
              {item.title}
            </button>
          ) : (
            <ListPieceTitle>{item.title}</ListPieceTitle>
          )
        }
      />
      {block.more ? (
        <Button className="aw-block__more" onClick={() => void onAction({ id: block.more!.action.id })} size="sm">
          {block.more.label ?? 'Show more'}
        </Button>
      ) : null}
    </>
  )
}

function TableView({ block, onAction }: { block: TableBlock; onAction: PanelBlocksProps['onAction'] }) {
  const open = block.rowAction
  return (
    // On a narrow panel each row is a card, and each value says what it is (the declared table's way).
    <div className="aw-block__scroll" data-scroll-x="">
      <table className="aw-piece__table">
        <thead>
          <tr>
            {block.columns.map((column) => (
              <th key={column.id} scope="col">
                {column.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {block.rows.map((row) => (
            <tr key={row.id}>
              {block.columns.map((column, index) => (
                <td data-label={column.label} key={column.id}>
                  {/* A row the module can open opens from its first value, as a list row opens from its title. */}
                  {open && index === 0 ? (
                    <button className="aw-block__rowbutton" onClick={() => void onAction({ id: open.id, values: { row: row.id } })} type="button">
                      <Value kind={column.kind} value={row.cells[column.id]} />
                    </button>
                  ) : (
                    <Value kind={column.kind} value={row.cells[column.id]} />
                  )}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function ButtonsView({ block, onAction }: { block: ButtonsBlock; onAction: PanelBlocksProps['onAction'] }) {
  const [asking, setAsking] = useState<ButtonsBlock['items'][number] | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const run = (item: ButtonsBlock['items'][number]) => {
    setBusy(item.action.id)
    void Promise.resolve(onAction({ id: item.action.id })).finally(() => {
      setBusy(null)
      setAsking(null)
    })
  }
  const press = (item: ButtonsBlock['items'][number]) => (item.confirm ? setAsking(item) : run(item))
  // More than two: the first stays a button and the rest fold into a menu, as on a narrow panel.
  const [first, second, ...rest] = block.items
  const shown = rest.length ? [first] : [first, second].filter(Boolean)
  const folded = rest.length ? [second, ...rest] : []
  const menu: MenuEntry[] = folded.map((item) => ({ label: item.label, danger: item.style === 'danger', onSelect: () => press(item) }))
  return (
    <div className="aw-block__buttons">
      {shown.map((item) => (
        <Button busy={busy === item.action.id} key={item.action.id + item.label} onClick={() => press(item)} variant={item.style ?? 'secondary'}>
          {item.label}
        </Button>
      ))}
      {menu.length ? <Menu items={menu} label="More actions" trigger={<IconButton icon="more" label="More actions" />} /> : null}
      <ConfirmDialog
        busy={asking !== null && busy === asking.action.id}
        confirmLabel={asking?.label ?? ''}
        onClose={() => setAsking(null)}
        onConfirm={() => asking && run(asking)}
        open={asking !== null}
        title={asking?.confirm?.title ?? ''}
        tone={asking?.style === 'danger' ? 'danger' : 'primary'}
      >
        {asking?.confirm?.body}
      </ConfirmDialog>
    </div>
  )
}

const STATE: Record<StatusBlock['state'], { tone: PanelTone; word: string }> = {
  ok: { tone: 'success', word: 'Done' },
  working: { tone: 'agent', word: 'Working' },
  waiting: { tone: 'warning', word: 'Waiting' },
  error: { tone: 'danger', word: 'Failed' },
  unknown: { tone: 'muted', word: 'Unknown' },
}

function StatusView({ block }: { block: StatusBlock }) {
  const state = STATE[block.state] ?? STATE.unknown
  const progress = typeof block.progress === 'number' ? Math.min(1, Math.max(0, block.progress)) : null
  const id = useId()
  return (
    <div className="aw-block__status">
      <span className={`aw-block__dot is-${state.tone}`} aria-hidden="true" />
      <span className="aw-block__statustext">
        <span className="visually-hidden">{state.word}: </span>
        <span id={id}>{block.label}</span>
        {block.detail ? <span className="aw-block__detail">{block.detail}</span> : null}
      </span>
      {progress !== null ? (
        <progress aria-labelledby={id} className="aw-block__progress" max={1} value={progress}>
          {Math.round(progress * 100)}%
        </progress>
      ) : null}
    </div>
  )
}

function ChartView({ block }: { block: ChartBlock }) {
  if (block.kind === 'number') {
    return (
      <p className="aw-block__figure">
        {typeof block.value === 'number' ? block.value.toLocaleString() : '—'}
        {block.unit ? <span className="aw-block__unit"> {block.unit}</span> : null}
      </p>
    )
  }
  const style = block.kind
  const series = block.series ?? []
  const x = block.x ?? []
  return (
    // Each series is its own figure, and a wide one scrolls inside itself.
    <div className="aw-block__scroll" data-scroll-x="">
      <Suspense fallback={<p className="aw-block__text is-muted">Loading the chart…</p>}>
        {series.map((one) => (
          <figure className="aw-block__chart" key={one.name}>
            <figcaption className="aw-block__caption">{one.name}</figcaption>
            <ChartFigure
              additive={false}
              label={one.name}
              points={x.map((label, index) => ({ key: label, label, color: null, count: 0, value: one.values[index] ?? null }))}
              style={style}
            />
          </figure>
        ))}
      </Suspense>
    </div>
  )
}
