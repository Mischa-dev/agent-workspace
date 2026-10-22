import type { CSSProperties, ReactNode } from 'react'

import { Button, Checkbox, Icon, SegmentedControl } from '../../../src/ui'
import { agents, card, tasks, type StatusKey } from '../data'
import { ease, mix, opening, progress } from '../motion'
import { Cursor, pointerAt, pressed, type Waypoint } from '../parts/Cursor'
import { useBoxes } from '../parts/measure'
import { AppWindow, ProjectSidebar, Shell } from '../parts/Shell'
import { CarriedCard, TaskBoard, TaskList, TasksHeader, TasksToolbar, type BoardColumn } from '../parts/Tasks'
import { Caption } from '../parts/Text'

/*
 * 0:12. A plan comes in, becomes tasks, and the list turns into a board where
 * work moves. Everything inside the window is the app's own task UI.
 */

const PLAN = [
  '## 1.4 launch',
  '- [ ] Export tasks as CSV',
  '  - [ ] Write the CSV encoder',
  '  - [ ] Add an Export button',
  '  - [ ] Document the format',
  '- [ ] Design the Export button',
  '- [ ] Bulk edit from the board',
  '- [ ] Webhooks for task events',
  '- [ ] Retry failed webhook deliveries',
  '- [ ] Offline drafts on phones',
  '- [ ] Fix stale unread badges',
  '- [ ] Import plans from Markdown',
  '- [ ] Usage page for each project',
  '- [ ] Write the export docs',
  '- [ ] Reminders on any message',
  '- [x] Faster board on big projects',
  '- [x] Keyboard moves on the board',
  '- [x] Agree the column order',
]

const LIST_ORDER = [12, 26, 27, 28, 14, 15, 16, 17, 18, 19, 20, 21, 24, 25, 22, 23, 9]

/** The project's window. The task scene goes on in this same window, so the two share its size. */
export const WIN = { w: 1580, h: 960 }
// Where the window sits: beside the caption while the plan comes in, then
// centered at full size once it's a board.
const SIDE = { x: 716, y: 176, k: 0.74 }
const FULL = { x: 170, y: 60, k: 1 }

// Beats inside the scene.
const T = {
  dialog: 0.9,
  typeFrom: 1.15,
  importClick: 2.75,
  rowsFrom: 3.0,
  boardClick: 5.0,
  grab: 6.15,
  drop: 7.15,
  open: 7.85,
}

/** Where the task scene takes over, in this scene's time: just after NW-12 is clicked. */
export const PLAN_HANDOVER = 8.0

/** The camera on the window: beside the caption, then centered as the board appears, creeping in while work moves. */
export function planCamera(t: number) {
  const push = progress(t, T.boardClick + 0.05, 1.5, ease.inOut)
  const creep = 1 + 0.03 * progress(t, T.boardClick + 1.2, T.open - T.boardClick - 1.2, ease.inOut)
  return {
    k: mix(SIDE.k, FULL.k, push) * creep,
    x: 960 - (960 - mix(SIDE.x, FULL.x, push)) * creep,
    y: 540 - (540 - mix(SIDE.y, FULL.y, push)) * creep,
  }
}

/** The project's agents in the sidebar, as they are while the plan comes in and NW-12 opens. */
export const PLAN_AGENTS = [
  { actor: agents.codex, state: 'idle', status: 'Ready for work' },
  { actor: agents.claude, state: 'working', status: 'Fixing NW-18' },
  { actor: agents.muse, state: 'working', status: 'Designing NW-14' },
] as const

export const PLAN_CHANNELS = [{ name: 'general' }, { name: 'launch', unread: true }, { name: 'design' }]

/** The board's columns, before and after NW-15 moves to In progress, and while it's carried (a slot in both). */
export function boardColumns(dropped: boolean | 'moving'): BoardColumn[] {
  return (
    [
      ['backlog', [16, 17, 21]],
      ['ready', dropped === true ? [12, 24] : [15, 12, 24]],
      ['progress', dropped === false ? [14, 18, 25] : [15, 14, 18, 25]],
      ['review', [19, 20]],
    ] as [StatusKey, number[]][]
  ).map(([status, numbers]) => ({
    status,
    cards: numbers.map((n) => {
      const id = n === 15 ? (status === 'progress' ? 'p15' : 'c15') : `c${n}`
      return { ...card(tasks[n], status), id }
    }),
  }))
}

/**
 * The Tasks page in the main column: its header and toolbar over the list or
 * the board. `over` is a layer drawn on top of the whole page (the task scene
 * opens NW-12 in it), so both scenes lay the page out the same way.
 */
export function TasksPage({ children, over, view }: { children: ReactNode; over?: ReactNode; view: 'list' | 'board' }) {
  return (
    <div className="tr-pagestack">
      <div className={view === 'board' ? 'aw-page aw-page--wide aw-workpage aw-workpage--board' : 'aw-page aw-page--wide aw-workpage'}>
        <TasksHeader view={view} />
        <TasksToolbar />
        {children}
      </div>
      {over}
    </div>
  )
}

export function Plan({ t }: { t: number }) {
  // The page becomes the board as Board is pressed; the list's rows fall away over the board's first beat.
  const view = t < T.boardClick + 0.05 ? 'list' : 'board'
  const listOut = (index: number) => 1 - progress(t, T.boardClick + 0.02 + index * 0.012, 0.22)
  const dropped = t >= T.drop
  const m = useBoxes('plan', {
    importGo: '.tr-import-go',
    boardSeg: '.aw-seg__opt[data-value="board"]',
    readyFirst: '[data-drag-id="c15"]',
    progressCell: '.aw-tboard__cell:nth-of-type(3)',
    progressHead: '.aw-tboard__head:nth-of-type(3)',
    card12: '[data-drag-id="c12"]',
    landed: '[data-drag-id="p15"]',
    carried: '.tr-carried .aw-tcard',
  })

  const carriedSeed = tasks[15]
  const carrying = t >= T.grab && t < T.drop + 0.22
  // While NW-15 is carried, Ready closes up where it was and In progress opens a slot where it'll land.
  const columns = boardColumns(carrying ? 'moving' : dropped)
  const slot = m.box('readyFirst')
  // The card's own height, from the carried copy, since its slot on the board is closing.
  const natural = m.box('carried').h || slot.h
  const slots = carrying
    ? {
        c15: { s: 1 - progress(t, T.grab + 0.3, 0.35, ease.inOut), h: natural },
        p15: { s: progress(t, T.grab + 0.5, 0.35, ease.inOut), h: natural },
      }
    : undefined
  const listCards = LIST_ORDER.map((n) => card(tasks[n]))
  const listIn = (index: number) => progress(t, T.rowsFrom + index * 0.06, 0.4)

  // The pointer: into the dialog, onto Import, up to Board, onto a card, across, then onto NW-12.
  const grabBox = { ...slot, h: natural }
  const grabAt = { x: grabBox.x + grabBox.w / 2, y: grabBox.y + grabBox.h / 2 - 10 }
  const cellBox = m.box('progressCell')
  const dropAt = { x: cellBox.x + cellBox.w / 2, y: cellBox.y + 60 }
  const path: Waypoint[] = [
    { t: 1.9, x: 1250, y: 860 },
    { t: T.importClick - 0.05, ...m.center('importGo') },
    { t: T.boardClick - 0.55, ...m.center('importGo', 40, 30) },
    { t: T.boardClick - 0.05, ...m.center('boardSeg') },
    { t: T.grab - 0.1, ...grabAt },
    { t: T.drop - 0.05, ...dropAt },
    { t: T.open - 0.45, x: dropAt.x - 60, y: dropAt.y + 40 },
    { t: T.open - 0.05, ...m.center('card12', -30, -12) },
  ]
  const clicks = [T.importClick, T.boardClick, T.grab, T.drop, T.open]

  // The carried card follows the pointer, tilted a little, then settles into its new slot.
  const pointer = pointerAt(path, t)
  const landed = m.box('landed')
  const settle = progress(t, T.drop, 0.22)
  const carriedPos = t < T.drop
    ? { x: pointer.x - (grabAt.x - grabBox.x), y: pointer.y - (grabAt.y - grabBox.y) }
    : { x: mix(pointer.x - (grabAt.x - grabBox.x), landed.x, settle), y: mix(pointer.y - (grabAt.y - grabBox.y), landed.y, settle) }
  const lift = t < T.drop ? progress(t, T.grab, 0.18) : 1 - settle

  // The camera: the window slides to the middle and grows as the board appears,
  // and creeps in while work moves. The task scene picks it up from there.
  const { k, x, y } = planCamera(t)
  const arrive = progress(t, 0.15, 0.9)
  const windowStyle: CSSProperties = {
    left: 0,
    top: 0,
    width: WIN.w,
    height: WIN.h,
    transformOrigin: '0 0',
    opacity: arrive,
    transform: `translate3d(${x}px, ${y + (1 - arrive) * 70}px, 0) scale(${k})`,
  }

  const dialogIn = progress(t, T.dialog, 0.35)
  const dialogOut = progress(t, T.importClick + 0.12, 0.25, ease.inOut)
  const showDialog = t >= T.dialog && dialogOut < 1

  return (
    <div className="tr-layer">
      <Caption
        at={0.25}
        exitAt={T.boardClick - 0.2}
        exitTo={{ x: -90 }}
        kicker="Tasks"
        kickerIcon={<Icon name="work" size={26} />}
        sub={['Bring a plan in from Markdown, Linear, Jira or GitHub.', 'List, board, timeline and graph stay in step.']}
        subAt={1.1}
        t={t}
        title={['Plan the', 'whole project.']}
        width={580}
        x={110}
        y={300}
      />
      <div style={{ position: 'absolute', inset: 0 }}>
        <div className="tr-abs" ref={m.root} style={windowStyle}>
          <AppWindow height={WIN.h} width={WIN.w}>
            <Shell sidebar={<ProjectSidebar active="tasks" agents={[...PLAN_AGENTS]} channels={PLAN_CHANNELS} />}>
              <TasksPage view={view}>
                {view === 'list' ? (
                  <TaskList cards={listCards} reveal={listIn} />
                ) : (
                  <>
                    {listOut(LIST_ORDER.length) > 0 ? (
                      <div className="tr-leaving" style={{ opacity: 1 - progress(t, T.boardClick + 0.1, 0.25) }}>
                        <TaskList cards={listCards} reveal={(index) => listIn(index) * listOut(index)} />
                      </div>
                    ) : null}
                    <TaskBoard
                      columns={columns}
                      doneCount={3}
                      reveal={(column, index) => progress(t, T.boardClick + 0.12 + column * 0.09 + index * 0.06, 0.45)}
                      slots={slots}
                      style={{ opacity: progress(t, T.boardClick + 0.05, 0.3) }}
                      target={t >= T.grab + 0.4 && t < T.drop ? 'progress' : undefined}
                    />
                  </>
                )}
              </TasksPage>
            </Shell>
          </AppWindow>
          {carrying ? (
            <CarriedCard
              card={card(carriedSeed, dropped ? 'progress' : 'ready')}
              lift={lift}
              style={{
                left: carriedPos.x,
                top: carriedPos.y,
                transform: `rotate(${2.5 * lift}deg) scale(${1 + 0.03 * lift})`,
                zIndex: 30,
              }}
              width={grabBox.w}
            />
          ) : null}
          {showDialog ? <ImportDialog in={dialogIn} out={dialogOut} pressed={pressed(clicks, t)} t={t} /> : null}
          <Cursor clicks={clicks} path={path} scale={1.6} showAt={1.9} t={t} />
        </div>
      </div>
    </div>
  )
}

// The plan arrives as a paste: a line at a time, quickly, and the button counts the tasks in it.
const LINE_EVERY = 0.045

function ImportDialog({ in: k, out, pressed: down, t }: { in: number; out: number; pressed: boolean; t: number }) {
  const shown = Math.max(0, Math.min(PLAN.length, Math.floor((t - T.typeFrom) / LINE_EVERY)))
  const lines = PLAN.slice(0, shown)
  const count = lines.filter((line) => line.trim().startsWith('- [')).length
  return (
    <>
      <div className="aw-scrim" style={{ position: 'absolute', inset: 0, zIndex: 40, opacity: opening(k) * (1 - out) * 0.6, borderRadius: 'var(--radius-lg)' }} />
      <div
        className="aw-dialog"
        style={{
          position: 'absolute',
          zIndex: 41,
          left: 628,
          top: 110,
          width: 660,
          opacity: opening(k) * (1 - out),
          transform: `translate3d(0, ${(1 - k) * 12}px, 0) scale(${mix(0.985, 1, k) * mix(1, 0.97, out)})`,
        }}
      >
        <header className="aw-dialog__head">
          <h2 className="aw-dialog__title">Import a plan</h2>
          <span className="aw-iconbtn aw-iconbtn--sm">
            <Icon name="x" size={16} />
          </span>
        </header>
        <p className="aw-dialog__desc">Bring tasks into Northwind from a Markdown plan, a CSV from Linear or Jira, or GitHub issues.</p>
        <div className="aw-dialog__body">
          <SegmentedControl
            label="Import from"
            onChange={() => undefined}
            options={[
              { value: 'md', label: 'Markdown plan' },
              { value: 'csv', label: 'CSV' },
              { value: 'gh', label: 'GitHub issues' },
            ]}
            value="md"
          />
          <div className="aw-field">
            <span className="aw-field__label">Plan</span>
            <div
              className="aw-input tr-plan"
              style={{
                height: 384,
                padding: 'var(--space-2) var(--space-3)',
                font: 'var(--t-code)',
                whiteSpace: 'pre',
                overflow: 'hidden',
                border: '1px solid var(--focus-ring)',
                borderRadius: 'var(--radius-sm)',
                background: 'var(--content-bg)',
              }}
            >
              {lines.join('\n')}
              <span style={{ opacity: Math.floor(t * 2.5) % 2 ? 0 : 1, color: 'var(--accent)' }}>▏</span>
            </div>
            <p className="aw-field__note">Each list item becomes a task; indented items become subtasks. “- [x]” items arrive done.</p>
          </div>
          <Checkbox checked label="Headings become milestones" onChange={() => undefined} />
        </div>
        <footer className="aw-dialog__foot">
          <Button>Cancel</Button>
          <Button className={down && t < T.importClick + 0.2 ? 'tr-import-go tr-pressed' : 'tr-import-go'} icon="check" variant="primary">
            {count ? `Import ${count} tasks` : 'Import'}
          </Button>
        </footer>
      </div>
    </>
  )
}
