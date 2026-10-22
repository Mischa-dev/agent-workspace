import type { CSSProperties, ReactNode } from 'react'

import { Avatar, Button, Icon, IconButton, Row, RowList, Section, StatusChip } from '../../../src/ui'
import { Markdown } from '../../../src/ui/markdown'
import { agents, card, people, tasks, type Actor, type StatusKey } from '../data'
import { opening } from '../motion'
import { Arrive } from './Chat'
import { Swap } from './Swap'
import { Criterion, TaskProps, TaskRef, TaskSection } from './Tasks'

/*
 * NW-12's own page, laid out as the app lays out a task on a wide screen: the
 * text and its sections in the middle, properties and starting work in the
 * rail. Each part's arrival is set by the caller.
 */

const DESCRIPTION = `Let people download any task view as a CSV file, with the columns in the order we agreed in NW-9.

Big projects should stream the rows instead of loading them all at once.`

const SUBTASKS: { key: string; title: string; who: Actor }[] = [
  { key: 'NW-26', title: 'Write the CSV encoder', who: agents.codex },
  { key: 'NW-27', title: 'Add an Export button', who: agents.muse },
  { key: 'NW-28', title: 'Document the format', who: agents.grok },
]

const CRITERIA = [
  'Exports what the current view shows, with its filters and sort',
  'Columns follow the order agreed in NW-9',
  'A 10,000-task project exports in under two seconds',
  'Dates use each person’s own time zone',
]

export type TaskPageState = {
  /** 0 to 1 for each part: how far it has arrived. */
  reveal: (part: TaskPart) => number
  status?: StatusKey
  /** The status it's changing from, and how far the change has gone. */
  statusChange?: { from: StatusKey; k: number }
  subtasksDone?: number
  criteriaDone?: number
  /** The add row's whiteboard item, while it's being pointed at or pressed. */
  whiteboardItem?: 'hover' | 'pressed'
  menu?: ReactNode
  whiteboard?: number
  start?: ReactNode
}

export type TaskPart = 'head' | 'text' | 'subtasks' | 'deps' | 'criteria' | 'props' | 'start' | 'activity'

function arrive(k: number, y = 14): CSSProperties {
  return { opacity: k, transform: `translate3d(0, ${(1 - k) * y}px, 0)` }
}

export function TaskPage({ criteriaDone = 0, menu, reveal, start, status = 'ready', statusChange, subtasksDone = 0, whiteboard, whiteboardItem }: TaskPageState) {
  const task = card(tasks[12], status)
  const linked = whiteboard !== undefined && whiteboard > 0
  return (
    <div className="aw-task aw-task--page is-wide">
      <div className="aw-task__bar">
        <nav aria-label="Breadcrumb" className="aw-crumbs">
          <span className="aw-crumbs__link">
            <Icon name="work" size={14} />
            Tasks
          </span>
          <Icon className="aw-crumbs__sep" name="chevron-right" size={14} />
          <span aria-current="page" className="aw-crumbs__here">
            NW-12
          </span>
        </nav>
        <span className="aw-task__spacer" />
        <span className="aw-menuanchor">
          <IconButton icon="more" label="Task actions" />
        </span>
      </div>
      <div className="aw-task__body">
        <div className="aw-task__column">
          <div className="aw-task__head" style={arrive(reveal('head'))}>
            <p className="aw-task__heading">Export tasks as CSV</p>
          </div>
          {/* Above the sections after it while its menu is open, as the menu's own layer is in the app. */}
          <div className="aw-task__text" style={{ ...arrive(reveal('text')), position: 'relative', zIndex: menu ? 2 : undefined }}>
            <div className="aw-taskdesc">
              <Markdown className="aw-taskdesc__body" source={DESCRIPTION} taskHref={() => '#'} />
            </div>
            <div aria-label="Add to this task" className="aw-taskadd tr-taskadd" role="group">
              <span aria-hidden="true" className="aw-taskadd__label">
                Add
              </span>
              <button className="aw-taskadd__item" type="button">
                link
              </button>
              <span aria-hidden="true" className="aw-taskadd__dot">
                ·
              </span>
              <button className="aw-taskadd__item" type="button">
                file
              </button>
              {/* Once a whiteboard is linked, the add row stops offering one: the item fades as the section opens. */}
              {whiteboard !== undefined && whiteboard >= 1 ? null : (
                <span style={{ display: 'contents' }}>
                  <span aria-hidden="true" className="aw-taskadd__dot" style={linked ? { opacity: 1 - whiteboard } : undefined}>
                    ·
                  </span>
                  <span className="aw-menuanchor" style={linked ? { opacity: 1 - whiteboard } : undefined}>
                    <button
                      className={['aw-taskadd__item', 'tr-addboard', whiteboardItem === 'hover' ? 'tr-hover' : '', whiteboardItem === 'pressed' ? 'tr-hover tr-pressed' : '']
                        .filter(Boolean)
                        .join(' ')}
                      type="button"
                    >
                      whiteboard
                    </button>
                    {menu}
                  </span>
                </span>
              )}
            </div>
          </div>
          <div style={arrive(reveal('subtasks'))}>
            <TaskSection count={`${subtasksDone}/${SUBTASKS.length}`} title="Subtasks">
              <ul className="aw-taskrefs">
                {SUBTASKS.map((sub, index) => (
                  <TaskRef assignee={sub.who} key={sub.key} keyLabel={sub.key} status={index < subtasksDone ? 'done' : 'ready'} title={sub.title} />
                ))}
              </ul>
            </TaskSection>
          </div>
          <div style={arrive(reveal('deps'))}>
            <TaskSection title="Dependencies">
              <div className="aw-deps">
                <div className="aw-deps__group">
                  <h3 className="aw-deps__title">Blocked by</h3>
                  <ul className="aw-taskrefs">
                    <TaskRef assignee={people.leo} keyLabel="NW-9" status="done" title={tasks[9].title} />
                  </ul>
                </div>
                <div className="aw-deps__group">
                  <h3 className="aw-deps__title">Blocks</h3>
                  <ul className="aw-taskrefs">
                    <TaskRef assignee={agents.grok} keyLabel="NW-21" status="backlog" title={tasks[21].title} />
                  </ul>
                </div>
              </div>
            </TaskSection>
          </div>
          <div style={arrive(reveal('criteria'))}>
            <TaskSection count={`${criteriaDone}/${CRITERIA.length}`} title="Acceptance criteria">
              <ol className="aw-criteria">
                {CRITERIA.map((text, index) => (
                  <Criterion checked={index < criteriaDone} key={text} text={text} />
                ))}
              </ol>
            </TaskSection>
          </div>
          {linked ? (
            <Arrive id="task-boards" k={whiteboard}>
              <div className="tr-boards">
              <Section
                actions={
                  <span className="aw-linked-boards__actions">
                    <Button icon="link" size="sm" variant="ghost">
                      Link a whiteboard
                    </Button>
                    <Button icon="plus" size="sm" variant="ghost">
                      New whiteboard
                    </Button>
                  </span>
                }
                title="Whiteboards"
              >
                <RowList label="Linked whiteboards">
                  <Row
                    actions={<IconButton icon="unlink" label="Unlink Export flow" size="sm" />}
                    className="tr-boardrow"
                    meta={<span>Changed 2 hours ago by Leo Martin</span>}
                    title={<span className="tr-link">Export flow</span>}
                  />
                </RowList>
              </Section>
              </div>
            </Arrive>
          ) : null}
          {/*
            The column's gap before the whiteboards opens with them: a grid row
            can't be shorter than nothing, so the section after them is pulled
            up by the gap still to open, and the sections under it move down
            smoothly from the first frame.
          */}
          <div style={{ ...arrive(reveal('activity')), ...(linked && whiteboard < 1 ? { marginTop: `calc(var(--space-6) * ${opening(whiteboard) - 1})` } : {}) }}>
            <TaskSection title="Activity">
              <ul className="aw-taskrefs">
                <li className="aw-taskref">
                  <Avatar kind="person" name={people.priya.name} seed={people.priya.id} size={20} />
                  <span className="aw-taskref__title">Priya Shah imported this from the 1.4 launch plan</span>
                </li>
              </ul>
            </TaskSection>
          </div>
        </div>
        <aside aria-label="Task details" className="aw-task__rail">
          <div style={arrive(reveal('props'))}>
            <TaskProps assignee={agents.codex} card={task} change={statusChange} status={status} />
          </div>
          <div style={arrive(reveal('start'))}>{start}</div>
          <p className="aw-task__dates" style={arrive(reveal('start'))}>
            Created 2 days ago · Updated just now
          </p>
        </aside>
      </div>
    </div>
  )
}

/** The whiteboard menu under the add row's "whiteboard", with one item pointed at. */
export function BoardMenu({ hover, k }: { hover?: 'new' | 'link'; k: number }) {
  return (
    <div
      className="aw-menu tr-menu"
      role="menu"
      style={{ position: 'absolute', top: 'calc(100% + 6px)', left: 0, opacity: k, transform: `translate3d(0, ${(1 - k) * -6}px, 0) scale(${0.98 + 0.02 * k})`, transformOrigin: 'top left' }}
    >
      <button className={hover === 'new' ? 'aw-menu__item tr-hover' : 'aw-menu__item'} role="menuitem" type="button">
        <Icon name="plus" />
        <span className="aw-menu__label">New whiteboard…</span>
      </button>
      <button className={hover === 'link' ? 'aw-menu__item tr-hover tr-boardlink' : 'aw-menu__item tr-boardlink'} role="menuitem" type="button">
        <Icon name="link" />
        <span className="aw-menu__label">Link a whiteboard…</span>
      </button>
    </div>
  )
}

/**
 * Starting work: whether it can start, and the button that starts an agent on
 * it. `readyOut` folds the "Ready to start" line away, gap and all, once the
 * work has started, and `latestIn` opens the latest launch under the text.
 */
export function StartCard({ latest, latestIn = 1, pressed, readyOut = 0 }: { latest?: ReactNode; latestIn?: number; pressed?: boolean; readyOut?: number }) {
  const latestOpen = latest && latestIn > 0 && latestIn < 1 ? opening(latestIn) : 1
  return (
    <section aria-label="Starting work" className="aw-taskstart">
      <Arrive id="start-ready" k={1} out={readyOut}>
        <p className="aw-taskstart__state is-ready">
          <Icon name="circle-check" size={16} />
          Ready to start
        </p>
      </Arrive>
      {/* The gap after the line closes with it, by pulling the launch card up. */}
      <section className="aw-launchcard is-embedded" style={readyOut > 0 && readyOut < 1 ? { marginTop: `calc(var(--space-3) * ${-opening(readyOut)})` } : undefined}>
        {/* The gap before the latest launch opens with it, by letting the text above hang into it. */}
        <p className="aw-sidecard__text" style={latestOpen < 1 ? { marginBottom: `calc(var(--space-2) * ${latestOpen - 1})` } : undefined}>
          Codex in Northwind cloud, on the project’s defaults. It works in the task’s thread.
        </p>
        <div className="aw-launchcard__actions">
          <Button className={pressed ? 'tr-launch tr-pressed' : 'tr-launch'} icon="play" variant="primary">
            Launch
          </Button>
          <IconButton icon="chevron-down" label="More launch options" size="sm" />
        </div>
        {latest ? (
          // Last, as the app orders it under the text.
          <Arrive id="start-latest" k={latestIn} style={{ order: 2 }}>
            {latest}
          </Arrive>
        ) : null}
      </section>
    </section>
  )
}

type LaunchState = 'starting' | 'working' | 'done'

function LaunchChip({ state }: { state: LaunchState }) {
  const chip =
    state === 'done'
      ? ({ label: 'Done', tone: 'success', icon: 'circle-check', spin: false } as const)
      : ({ label: state === 'working' ? 'Working' : 'Starting', tone: 'agent', icon: 'loader', spin: true } as const)
  return (
    <StatusChip icon={chip.icon} spin={chip.spin} tone={chip.tone}>
      {chip.label}
    </StatusChip>
  )
}

/** The latest launch under the button; `change` cross-fades its chip from the state it was in. */
export function LatestLaunch({ change, state }: { change?: { from: LaunchState; k: number }; state: LaunchState }) {
  return (
    <div className="aw-launchcard__latest">
      {change ? <Swap from={<LaunchChip state={change.from} />} id="launch-chip" inline k={change.k} to={<LaunchChip state={state} />} /> : <LaunchChip state={state} />}
      <span className="aw-sidecard__note">Codex · just now</span>
      <span className="aw-launchcard__link">Open thread</span>
    </div>
  )
}
