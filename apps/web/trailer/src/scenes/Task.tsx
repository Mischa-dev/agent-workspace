import type { CSSProperties } from 'react'

import { AvatarStack, Button, Icon, IconButton, StatusChip } from '../../../src/ui'
import { agents, people } from '../data'
import { clamp, ease, mix, progress } from '../motion'
import { Cursor, pointerAt, type Waypoint } from '../parts/Cursor'
import { useBoxes } from '../parts/measure'
import { AppWindow, ProjectSidebar, Shell, agentChange } from '../parts/Shell'
import { Collaborator, Drawn, Hand, type Shape } from '../parts/Sketch'
import { BoardMenu, StartCard, TaskPage, type TaskPart } from '../parts/TaskPage'
import { TaskBoard } from '../parts/Tasks'
import { Caption } from '../parts/Text'
import { PLAN_AGENTS, PLAN_CHANNELS, PLAN_HANDOVER, TasksPage, WIN, boardColumns, planCamera } from './Plan'

/*
 * 0:20. NW-12 opens out of its card, in the same window the board was in, and
 * the window steps aside for the caption: subtasks, dependencies and
 * acceptance criteria. Then a whiteboard gets pinned to it, and opens, with
 * an agent drawing on it.
 */

// The window as the board left it, and where it settles beside the caption.
const FROM = planCamera(PLAN_HANDOVER)
const SIDE = { x: 664, y: 162, k: 0.78 }

const T = {
  opening: 0.02,
  glide: 0.2,
  pointer: 1.9,
  boardItem: 2.8,
  linkItem: 3.45,
  linked: 3.6,
  open: 4.15,
  exit: 8.1,
}

// The task's parts arrive as its card finishes opening into the page.
const ARRIVALS: Record<TaskPart, number> = {
  head: 0.3,
  text: 0.38,
  props: 0.44,
  subtasks: 0.52,
  start: 0.58,
  deps: 0.64,
  criteria: 0.76,
  activity: 0.88,
}

// The whiteboard, once it's open: where it sits on screen and how big.
const BOARD = { x: 610, y: 120, w: 1220, h: 840 }

// The pointer is still on NW-12 from the board, its click just landing.
const HANDOFF_CLICK = 7.85 - PLAN_HANDOVER

export function Task({ t }: { t: number }) {
  const m = useBoxes('task', {
    boardItem: '.tr-addboard',
    linkItem: '.tr-boardlink',
    row: '.tr-boardrow',
    card12: '[data-drag-id="c12"]',
    page: '.tr-pagestack',
  })
  // The menu opens on a quick fade, and closes on one once its item is picked.
  const menuIn = progress(t, T.boardItem + 0.04, 0.16) * (1 - progress(t, T.linkItem + 0.1, 0.12))
  const menuOpen = t >= T.boardItem + 0.04 && menuIn > 0
  const linked = progress(t, T.linked, 0.45)

  const path: Waypoint[] = [
    { t: T.pointer, x: 760, y: 760 },
    { t: T.boardItem - 0.05, ...m.center('boardItem', -6, 2) },
    { t: T.linkItem - 0.06, ...m.center('linkItem', -30, 0) },
    { t: T.linked + 0.35, ...m.center('linkItem', 10, 120) },
  ]
  const clicks = [T.boardItem, T.linkItem]

  // The window glides from where the board had it to beside the caption, then steps back behind the whiteboard.
  const glide = progress(t, T.glide, 1.0, ease.inOut)
  const behind = progress(t, T.open, 0.7, ease.inOut)
  const k = mix(FROM.k, SIDE.k, glide) * mix(1, 0.96, behind)
  const x = mix(FROM.x, SIDE.x, glide)
  const y = mix(FROM.y, SIDE.y, glide)
  const windowStyle: CSSProperties = {
    left: 0,
    top: 0,
    width: WIN.w,
    height: WIN.h,
    transformOrigin: '0 0',
    opacity: (1 - behind) * (1 - progress(t, T.exit, 0.4, ease.inOut)),
    transform: `translate3d(${x}px, ${y}px, 0) scale(${k})`,
    filter: behind ? `blur(${behind * 2}px)` : undefined,
  }

  // NW-12's card opens into its page: a frame grows from the card to the whole column, and the page fills it.
  const opening = progress(t, T.opening, 0.6, ease.inOut)
  const card = m.box('card12')
  const page = m.box('page')
  const inset = {
    top: mix(card.y - page.y, 0, opening),
    left: mix(card.x - page.x, 0, opening),
    right: mix(page.x + page.w - card.x - card.w, 0, opening),
    bottom: mix(page.y + page.h - card.y - card.h, 0, opening),
  }
  const radius = mix(8, 0, opening)
  // The frame lifts off the board as it grows, and settles as it fills the column.
  const lift = Math.sin(Math.PI * opening)

  const reveal = (part: TaskPart) => progress(t, ARRIVALS[part], 0.5)
  const row = m.box('row')
  const from = { x: x + row.x * k, y: y + row.y * k, w: row.w * k, h: row.h * k }

  const taskPage = (
    <TaskPage
      menu={menuOpen ? <BoardMenu hover={t > T.linkItem - 0.25 ? 'link' : undefined} k={menuIn} /> : null}
      reveal={reveal}
      start={<StartCard />}
      whiteboard={t >= T.linked ? linked : undefined}
      whiteboardItem={t >= T.boardItem && t < T.boardItem + 0.16 ? 'pressed' : t > T.boardItem - 0.3 && t < T.linked ? 'hover' : undefined}
    />
  )

  return (
    <div className="tr-layer">
      <Caption
        at={0.75}
        exitAt={T.open - 0.15}
        exitTo={{ x: -90 }}
        sub="Subtasks, dependencies and acceptance criteria, so everyone can see what done looks like."
        t={t}
        title={['Every task holds', 'the whole plan.']}
        titleScale={2.5}
        width={520}
        x={110}
        y={330}
      />
      <Caption
        at={T.open + 0.25}
        exitAt={T.exit}
        exitTo={{ x: -90 }}
        kicker="Whiteboards"
        kickerIcon={<Icon name="whiteboard" size={26} />}
        sub="Pin one to a task, a project or a page. Agents can draw on it too."
        t={t}
        title={['Sketch it out.', 'Pin it to', 'the plan.']}
        titleScale={2.7}
        width={460}
        x={110}
        y={300}
      />
      <div className="tr-abs" ref={m.root} style={windowStyle}>
        <AppWindow height={WIN.h} width={WIN.w}>
          <Shell
            sidebar={
              <ProjectSidebar
                active="tasks"
                agents={[
                  PLAN_AGENTS[0],
                  PLAN_AGENTS[1],
                  agentChange(t, T.open + 1.5, agents.muse, PLAN_AGENTS[2], { state: 'working', status: 'Drawing on Export flow' }),
                ]}
                channels={PLAN_CHANNELS}
              />
            }
          >
            {/* The board stays under the page once it has opened, so nothing in the column moves when the frame goes. */}
            <TasksPage
              over={
                <>
                  <div className="tr-opening" style={{ background: 'transparent', clipPath: opening < 1 ? `inset(${inset.top}px ${inset.right}px ${inset.bottom}px ${inset.left}px round ${radius}px)` : undefined }}>
                    <div style={{ position: 'absolute', inset: 0, background: 'var(--content-bg)', opacity: clamp(opening / 0.12) }} />
                    {taskPage}
                  </div>
                  {opening < 1 ? (
                    <div
                      className="tr-opening__frame"
                      style={{
                        left: inset.left,
                        top: inset.top,
                        width: page.w - inset.left - inset.right,
                        height: page.h - inset.top - inset.bottom,
                        borderRadius: radius,
                        boxShadow: `0 ${10 * lift}px ${28 * lift}px color-mix(in oklch, var(--content-fg) ${Math.round(16 * lift)}%, transparent)`,
                        opacity: 1 - clamp((opening - 0.75) / 0.25),
                      }}
                    />
                  ) : null}
                </>
              }
              view="board"
            >
              <TaskBoard columns={boardColumns(true)} doneCount={3} reveal={() => 1} />
            </TasksPage>
          </Shell>
        </AppWindow>
        <Cursor clicks={[HANDOFF_CLICK]} hideAt={0.1} path={[{ t: -1, ...m.center('card12', -30, -12) }]} scale={1.6} showAt={-1} t={t} />
        <Cursor clicks={clicks} hideAt={T.open} path={path} scale={1.6} showAt={T.pointer} t={t} />
      </div>
      {t >= T.open - 0.05 ? <WhiteboardPanel from={from} t={t - T.open} fade={1 - progress(t, T.exit, 0.4, ease.inOut)} /> : null}
    </div>
  )
}

/* The whiteboard: the app's whiteboard bar over a canvas, sketched in rough.js. */

const SHAPES: { at: number; shape: Shape; label?: string; lx?: number; ly?: number }[] = [
  { at: 0.5, shape: { kind: 'rect', x: 70, y: 240, w: 220, h: 96, seed: 3 }, label: 'Pick a view', lx: 180, ly: 288 },
  { at: 0.85, shape: { kind: 'arrow', points: [[298, 288], [380, 288]], seed: 4 } },
  { at: 0.95, shape: { kind: 'rect', x: 390, y: 240, w: 220, h: 96, fill: 'var(--accent-soft)', seed: 5 }, label: 'Encode CSV', lx: 500, ly: 288 },
  { at: 1.3, shape: { kind: 'arrow', points: [[618, 288], [700, 288]], seed: 6 } },
  { at: 1.4, shape: { kind: 'rect', x: 710, y: 240, w: 220, h: 96, fill: 'var(--success-soft)', seed: 7 }, label: 'Download .csv', lx: 820, ly: 288 },
]

// Muse's part: a question under "Encode CSV" and the branch for big projects.
const MUSE: { at: number; shape: Shape; label?: string; lx?: number; ly?: number }[] = [
  { at: 1.75, shape: { kind: 'arrow', points: [[500, 344], [500, 418]], seed: 8 } },
  { at: 1.95, shape: { kind: 'diamond', x: 380, y: 428, w: 240, h: 136, fill: 'var(--warning-soft)', seed: 9 }, label: '10k+ tasks?', lx: 500, ly: 496 },
  { at: 2.4, shape: { kind: 'arrow', points: [[628, 496], [670, 502], [700, 502]], seed: 10 } },
  { at: 2.55, shape: { kind: 'rect', x: 710, y: 454, w: 220, h: 96, fill: 'var(--mention-bg)', seed: 11 }, label: 'Stream rows', lx: 820, ly: 502 },
]

// Where Muse's pen is while it draws.
const MUSE_PATH: Waypoint[] = [
  { t: 1.35, x: 960, y: 690 },
  { t: 1.75, x: 505, y: 350 },
  { t: 1.97, x: 505, y: 420 },
  { t: 2.4, x: 628, y: 500 },
  { t: 2.58, x: 700, y: 504 },
  { t: 3.05, x: 932, y: 550 },
  { t: 4.2, x: 900, y: 650 },
]

const LEO_PATH: Waypoint[] = [
  { t: 0.6, x: 1120, y: 690 },
  { t: 1.1, x: 1000, y: 190 },
  { t: 2.2, x: 1040, y: 220 },
  { t: 3.4, x: 300, y: 610 },
  { t: 4.2, x: 260, y: 650 },
]

function WhiteboardPanel({ fade, from, t }: { fade: number; from: { x: number; y: number; w: number; h: number }; t: number }) {
  const grow = progress(t, 0, 0.6, ease.inOut)
  const frame = {
    left: mix(from.x, BOARD.x, grow),
    top: mix(from.y, BOARD.y, grow),
    width: mix(from.w, BOARD.w, grow),
    height: mix(from.h, BOARD.h, grow),
  }
  const content = clamp((grow - 0.3) / 0.5)
  const drawing = t > 1.3 && t < 3.1
  const muse = pointerAt(MUSE_PATH, t)
  const leo = pointerAt(LEO_PATH, t)
  return (
    <div className="tr-panel" style={{ ...frame, opacity: clamp(grow * 8) * fade, zIndex: 5 }}>
      <div className="aw-wb" style={{ width: BOARD.w, height: BOARD.h, opacity: content }}>
        <header className="aw-wb__bar">
          <span className="aw-wb__back">
            <Icon name="arrow-left" size={16} />
            <span>Whiteboards</span>
          </span>
          <h1 className="aw-wb__title">Export flow</h1>
          <span className="aw-wb__place">
            <StatusChip icon="lock" tone="muted">
              Northwind
            </StatusChip>
          </span>
          <span className="aw-wb__save">Saved</span>
          <span className="aw-wb__spacer" />
          {drawing ? (
            <StatusChip dots tone="agent">
              Muse is drawing
            </StatusChip>
          ) : null}
          <span className="aw-wb__editors">
            <AvatarStack
              people={[
                { id: people.leo.id, name: people.leo.name, kind: 'person' },
                { id: agents.muse.id, name: agents.muse.name, kind: 'agent' },
              ]}
            />
          </span>
          <Button icon="link" size="sm">
            Linked (1)
          </Button>
          <IconButton icon="more" label="Whiteboard actions" size="sm" />
        </header>
        <div className="aw-wb__canvas tr-canvas">
          <svg height={BOARD.h - 52} style={{ position: 'absolute', inset: 0 }} viewBox={`0 0 ${BOARD.w} ${BOARD.h - 52}`} width={BOARD.w}>
            <Hand anchor="start" at={0.45} size={38} t={t} x={80} y={160}>
              NW-12 export flow
            </Hand>
            {[...SHAPES, ...MUSE].map((item) => (
              <g key={item.shape.seed}>
                <Drawn at={item.at} length={item.shape.kind === 'arrow' ? 0.3 : 0.5} shape={item.shape} t={t} />
                {item.label ? <Hand at={item.at + 0.35} t={t} x={item.lx ?? 0} y={item.ly ?? 0}>{item.label}</Hand> : null}
              </g>
            ))}
            <g style={{ opacity: progress(t, 0.9, 0.3), transform: `rotate(-2deg)`, transformOrigin: '1065px 220px' }}>
              <Drawn at={0.9} length={0.35} shape={{ kind: 'rect', x: 968, y: 146, w: 196, h: 150, fill: 'var(--highlight-bg)', seed: 12 }} t={t} />
              <Hand anchor="start" at={1.15} size={20} t={t} x={984} y={180}>
                Columns:
              </Hand>
              <Hand anchor="start" at={1.3} size={20} t={t} x={984} y={212}>
                key, title, status,
              </Hand>
              <Hand anchor="start" at={1.5} size={20} t={t} x={984} y={244}>
                assignee, due
              </Hand>
            </g>
          </svg>
          {t > 0.5 ? <Collaborator color="var(--swatch-green)" name="Leo Martin" opacity={progress(t, 0.5, 0.3)} x={leo.x} y={leo.y} /> : null}
          {t > 1.2 ? <Collaborator color="var(--swatch-orchid)" name="Muse" opacity={progress(t, 1.2, 0.3)} x={muse.x} y={muse.y} /> : null}
        </div>
      </div>
    </div>
  )
}
