import type { CSSProperties, ReactNode } from 'react'
import { ListFilter } from 'lucide-react'

import { Avatar, Button, Checkbox, Icon, PageHeader, PageTabs, SegmentedControl, TextField, actorKind } from '../../../src/ui'
import { Table, type TableRow } from '../../../src/ui/pickers'
import { BoardCard } from '../../../src/work-items/BoardCard'
import { AssigneeMark, DueDate, TaskBadges } from '../../../src/work-items/ListCells'
import { TaskGlyph } from '../../../src/work-items/TaskGlyph'
import { LIST_COLUMNS } from '../../../src/work-items/taskDisplay'
import type { WorkItemCard } from '../../../src/work-items/types'
import { WorkItemLabels } from '../../../src/work-items/WorkItemLabels'
import { PriorityIcon, StatusIcon } from '../../../src/work-items/WorkItemIcons'
import { project, statuses, type Actor, type StatusKey } from '../data'
import { Swap } from './Swap'

/*
 * The Tasks pages, put together from the app's own task components (the board
 * card, status and priority marks, labels, the kit Table) and its classes.
 */

const noop = () => undefined

export function TasksHeader({ view }: { view: 'list' | 'board' }) {
  return (
    <>
      <PageHeader
        actions={
          <>
            <SegmentedControl
              label="View"
              onChange={noop}
              options={[
                { value: 'list', label: 'List', icon: 'list' },
                { value: 'board', label: 'Board', icon: 'board' },
              ]}
              value={view}
            />
            <Button iconEnd="chevron-down">Views</Button>
            <Button icon="plus" variant="primary">
              New task
            </Button>
          </>
        }
        icon="work"
        title="Tasks"
      >
        Everything that needs doing in {project.name}.
      </PageHeader>
      <PageTabs
        current="tasks"
        label="Task views"
        tabs={[
          { id: 'tasks', label: 'Tasks', to: '#', icon: 'work' },
          { id: 'milestones', label: 'Milestones', to: '#', icon: 'checklist' },
          { id: 'timeline', label: 'Timeline', to: '#', icon: 'clock' },
          { id: 'planning', label: 'Planning', to: '#', icon: 'branch' },
        ]}
      />
    </>
  )
}

export function TasksToolbar({ filter }: { filter?: ReactNode }) {
  return (
    <div className="aw-worktools tr-worktools">
      <TextField aria-label="Search" className="aw-worktools__search" compact placeholder="Search by title or key" prefix={<Icon name="search" size={14} />} readOnly />
      <Button icon={ListFilter}>Filter</Button>
      {filter}
      <span className="aw-worktools__spacer" />
    </div>
  )
}

/** One list row's cells, as the list view draws them for someone who can't edit. */
function cells(card: WorkItemCard): Record<string, ReactNode> {
  return {
    select: (
      <label className="aw-tasks__check">
        <input aria-label={`Select ${card.key}`} className="aw-check__input" readOnly tabIndex={-1} type="checkbox" />
        <span aria-hidden="true" className="aw-check__box">
          <Icon name="check" size={12} />
        </span>
      </label>
    ),
    priority: <PriorityIcon priority={card.priority} />,
    key: <span className="aw-tasks__key">{card.key}</span>,
    status: <StatusIcon category={card.status.semanticCategory} label={card.status.name} />,
    title: <span className="aw-tasks__title">{card.title}</span>,
    details: <TaskBadges card={card} />,
    due: <DueDate card={card} />,
    assignee: <AssigneeMark card={card} />,
  }
}

/** The list, with each row's arrival set by the caller (opacity and lift per row). */
export function TaskList({ cards, reveal }: { cards: WorkItemCard[]; reveal: (index: number) => number }) {
  const rows: TableRow[] = cards.map((card, index) => ({ key: card.id, cells: cells(card), className: `tr-r${index}` }))
  const css = cards
    .map((_, index) => {
      const k = reveal(index)
      return `.tr-list .tr-r${index}{opacity:${k};transform:translate3d(0,${(1 - k) * 14}px,0)}`
    })
    .join('')
  return (
    <div className="aw-tasks tr-list">
      <style>{css}</style>
      <p className="aw-tasks__summary">{cards.filter((_, index) => reveal(index) > 0.5).length} tasks</p>
      <Table className="aw-tasks__table" columns={LIST_COLUMNS} label="Tasks" rows={rows} />
    </div>
  )
}

export type BoardColumn = { status: StatusKey; cards: WorkItemCard[] }

/** The board, with each card's arrival and any card being carried set by the caller. */
export function TaskBoard({
  columns,
  doneCount,
  reveal,
  hidden = [],
  slots = {},
  style,
  target,
}: {
  columns: BoardColumn[]
  /** Finished tasks, folded into the narrow column after the board as the app does by default. */
  doneCount?: number
  reveal: (column: number, index: number) => number
  /** Cards drawn elsewhere for now (one being dragged). */
  hidden?: string[]
  /**
   * Empty slots that open or close while a card is carried, as the board makes
   * room: `s` of the card's height `h`, the gap after it included.
   */
  slots?: Record<string, { s: number; h: number }>
  /** On the board's own box, which has to stay the page's direct child to fill it. */
  style?: CSSProperties
  target?: StatusKey
}) {
  const css = columns
    .flatMap((column, columnIndex) =>
      column.cards.map((card, index) => {
        const slot = slots[card.id]
        if (slot) {
          // A grid row can't be shorter than nothing, so the gap after the slot closes by pulling the next card up instead.
          return (
            `.tr-board [data-drag-id="${card.id}"]{opacity:0;height:${slot.h * slot.s}px;padding:0;border-width:0;overflow:hidden;margin:0}` +
            `.tr-board [data-drag-id="${card.id}"] + .aw-tcard{margin-top:calc(var(--space-2) * ${slot.s - 1})}`
          )
        }
        const k = hidden.includes(card.id) ? 0 : reveal(columnIndex, index)
        return `.tr-board [data-drag-id="${card.id}"]{opacity:${k};transform:translate3d(0,${(1 - k) * -26}px,0)}`
      }),
    )
    .join('')
  return (
    <div className="aw-tboard-wrap tr-board" style={style}>
      <style>{css}</style>
      <div className="aw-tboard">
        <div className="aw-tboard__body">
          <div className="aw-tboard__grid aw-tboard__grid--fill" style={{ '--aw-board-columns': columns.length } as CSSProperties}>
            {columns.map((column) => (
              <header className="aw-tboard__head" key={`head-${column.status}`}>
                <div className="aw-tboard__headrow">
                  <StatusIcon category={statuses[column.status].semanticCategory} />
                  <h2 className="aw-tboard__title">{statuses[column.status].name}</h2>
                  <span className="aw-tboard__count">{column.cards.filter((card) => !hidden.includes(card.id) && !slots[card.id]).length}</span>
                  <span className="aw-iconbtn aw-iconbtn--sm aw-tboard__add">
                    <Icon name="plus" size={16} />
                  </span>
                </div>
              </header>
            ))}
            {columns.map((column) => (
              <div className={column.status === target ? 'aw-tboard__cell is-target' : 'aw-tboard__cell'} key={`cell-${column.status}`}>
                <ol className="aw-tboard__cards">
                  {column.cards.map((card) => (
                    <BoardCard
                      card={card}
                      dragId={card.id}
                      draggable={false}
                      focused={false}
                      held={false}
                      key={card.id}
                      onFocus={noop}
                      onKeyDown={noop}
                      onOpen={noop}
                      onPick={noop}
                      onPointerDown={noop}
                      open={false}
                      selected={false}
                      showStatus={false}
                    />
                  ))}
                </ol>
              </div>
            ))}
          </div>
          {doneCount ? (
            <div className="aw-tboard__done">
              <Button className="aw-tboard__donebtn" icon="circle-check" size="sm" variant="ghost">
                Show done ({doneCount})
              </Button>
            </div>
          ) : null}
        </div>
      </div>
    </div>
  )
}

/**
 * A card lifted off the board while it's carried, drawn with the board card's
 * own classes. `lift` fades its held look (the ring, the tilt and the shadow)
 * as it settles, so it lands looking exactly like the card that replaces it.
 */
export function CarriedCard({ card, lift = 1, style, width }: { card: WorkItemCard; lift?: number; style?: CSSProperties; width: number }) {
  return (
    <div className="tr-carried" style={{ position: 'absolute', width, '--held': lift, ...style } as CSSProperties}>
      <span aria-hidden="true" className="tr-carried__shadow" style={{ opacity: lift }} />
      <ol className="aw-tboard__cards">
      <BoardCard
        card={card}
        dragId={`carried-${card.id}`}
        draggable={false}
        focused={false}
        held
        onFocus={noop}
        onKeyDown={noop}
        onOpen={noop}
        onPick={noop}
        onPointerDown={noop}
        open={false}
        selected={false}
        showStatus={false}
      />
      </ol>
    </div>
  )
}

/* The task page */

export function TaskSection({ actions, children, count, title }: { actions?: ReactNode; children: ReactNode; count?: ReactNode; title: string }) {
  return (
    <section className="aw-tsection">
      <header className="aw-tsection__head">
        <h2 className="aw-tsection__title">
          {title}
          {count !== undefined ? <span className="aw-tsection__count">{count}</span> : null}
        </h2>
        {actions ?? (
          <div className="aw-tsection__actions">
            <span className="aw-iconbtn aw-iconbtn--sm">
              <Icon name="plus" size={16} />
            </span>
          </div>
        )}
      </header>
      {children}
    </section>
  )
}

export function TaskRef({
  assignee,
  keyLabel,
  status,
  style,
  title,
}: {
  assignee?: Actor
  keyLabel: string
  status: StatusKey
  style?: CSSProperties
  title: string
}) {
  const finished = status === 'done'
  return (
    <li className={finished ? 'aw-taskref is-finished' : 'aw-taskref'} style={style}>
      <StatusIcon category={statuses[status].semanticCategory} label={statuses[status].name} />
      <span className="aw-taskref__key">{keyLabel}</span>
      <span className="aw-taskref__title">{title}</span>
      {assignee ? <Avatar kind={assignee.kind} name={assignee.name} seed={assignee.id} size={20} /> : null}
    </li>
  )
}

export function Criterion({ checked, text, style }: { checked: boolean; text: string; style?: CSSProperties }) {
  return (
    <li className="aw-criterion" style={style}>
      <Checkbox checked={checked} label={text} onChange={noop} />
    </li>
  )
}

export function Prop({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="aw-prop">
      <span className="aw-prop__label">{label}</span>
      <div className="aw-prop__value">
        <span className="aw-prop__button is-static">{children}</span>
      </div>
    </div>
  )
}

function StatusValue({ status }: { status: StatusKey }) {
  return (
    <span className="tr-propvalue">
      <StatusIcon category={statuses[status].semanticCategory} />
      {statuses[status].name}
    </span>
  )
}

/** The task's properties; `change` cross-fades the status from the one it had. */
export function TaskProps({
  assignee,
  card,
  change,
  status,
}: {
  assignee: Actor
  card: WorkItemCard
  change?: { from: StatusKey; k: number }
  status: StatusKey
}) {
  return (
    <div className="aw-taskprops aw-taskprops--rail">
      <Prop label="Status">
        {change ? <Swap from={<StatusValue status={change.from} />} id="task-status" inline k={change.k} to={<StatusValue status={status} />} /> : <StatusValue status={status} />}
      </Prop>
      <Prop label="Priority">
        <PriorityIcon decorative priority={card.priority} />
        High
      </Prop>
      <Prop label="Assignee">
        <Avatar kind={assignee.kind} name={assignee.name} seed={assignee.id} size={18} />
        <span className="aw-prop__text">{assignee.name}</span>
      </Prop>
      <Prop label="Labels">
        <WorkItemLabels labels={card.labels} />
      </Prop>
      <Prop label="Milestone">
        <TaskGlyph name="milestone" />
        <span className="aw-prop__text">{card.milestone?.name}</span>
      </Prop>
      <Prop label="Due date">Nov 6</Prop>
      <Prop label="Estimate">
        <TaskGlyph name="estimate" />3 points
      </Prop>
    </div>
  )
}

export { actorKind }
