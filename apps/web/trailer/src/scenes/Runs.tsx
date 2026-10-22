import type { CSSProperties } from 'react'

import { Avatar, Button, Icon, StatusChip } from '../../../src/ui'
import { agents, message, people } from '../data'
import { clamp, ease, mix, progress } from '../motion'
import { Arrive, Message, Reactions, reaction, Timeline } from '../parts/Chat'
import { Cursor, pressed, type Waypoint } from '../parts/Cursor'
import { useBoxes } from '../parts/measure'
import { RUN_BRANCH, RUN_DONE_AT, RUN_PR, RunPage } from '../parts/RunPage'
import { AppWindow, ProjectSidebar, Shell, agentChange } from '../parts/Shell'
import { Swap } from '../parts/Swap'
import { LatestLaunch, StartCard, TaskPage } from '../parts/TaskPage'
import { Caption } from '../parts/Text'

/*
 * 0:57. Codex takes NW-12: one press of Launch, then the run's own page while
 * it works, step by step, and the pull request it reports back in the task's
 * thread.
 */

const WIN = { w: 1300, h: 940, x: 690, y: 92, k: 0.92 }

const T = {
  pointer: 0.75,
  launch: 1.35,
  starting: 1.45,
  working: 2.05,
  leave: 2.7,
  page: 2.85,
  behind: 2.85 + RUN_DONE_AT + 0.65,
  exit: 10.1,
}

// The task page slides out to the left as the run's page comes in from the right.
const SWITCH = 0.5
const SLIDE = 90

// The report, relative to the window stepping back.
const R = { message: 0.3, card: 0.6, cheer: 1.5, approved: 2.15 }

// The slice of the task's thread that comes forward: Codex's report and the run card under it.
const PANEL = { w: 600, h: 420, x: 860, y: 246, k: 1.4 }

const REPORT = message(40, agents.codex, '09:26', [
  'Opened a pull request for ',
  { task: 12, state: 'review' },
  '. 24 tests pass, and a 10,000-task project exports in 1.4 seconds. It’s ready for review.',
])

const SIDEBAR_CHANNELS = [{ name: 'general' }, { name: 'launch', unread: true }, { name: 'design' }]

export function Runs({ t }: { t: number }) {
  const m = useBoxes('runs', { launch: '.tr-launch' })

  const path: Waypoint[] = [
    { t: T.pointer, x: 860, y: 900 },
    { t: T.launch - 0.05, ...m.center('launch', -14, 4) },
    { t: T.leave - 0.3, ...m.center('launch', 70, 120) },
  ]
  const clicks = [T.launch]

  const arrive = progress(t, 0, 0.55)
  const behind = progress(t, T.behind, 0.7, ease.inOut)
  // A slow push the whole time the window is up, grown about the window's middle.
  const creep = 1 + 0.025 * progress(t, 0, T.behind + 0.7, ease.linear)
  const k = WIN.k * mix(0.97, 1, arrive) * mix(1, 0.96, behind) * creep
  const middle = { x: WIN.x + (WIN.w * WIN.k) / 2, y: WIN.y + (WIN.h * WIN.k) / 2 }
  const scale = k / WIN.k
  const windowStyle: CSSProperties = {
    left: 0,
    top: 0,
    width: WIN.w,
    height: WIN.h,
    transformOrigin: '0 0',
    opacity: arrive * (1 - behind),
    transform: `translate3d(${middle.x - (middle.x - WIN.x) * scale + (1 - arrive) * 30}px, ${middle.y - (middle.y - WIN.y) * scale}px, 0) scale(${k})`,
    filter: behind ? `blur(${behind * 2}px)` : undefined,
  }

  const launched = t >= T.starting
  const change = progress(t, T.leave, SWITCH, ease.inOut)
  const onRunPage = t >= T.leave
  const taskShown = change < 1

  return (
    <div className="tr-layer">
      <Caption
        at={0.3}
        exitAt={T.leave - 0.2}
        exitTo={{ x: -90 }}
        kicker="Runs"
        kickerIcon={<Icon name="activity" size={26} />}
        sub="One click starts Codex, Claude Code or your own agent on the task."
        t={t}
        title={['Hand it to', 'an *agent.*']}
        titleScale={2.6}
        width={520}
        x={110}
        y={330}
      />
      <Caption
        at={T.page + 0.1}
        exitAt={T.behind - 0.2}
        exitTo={{ x: -90 }}
        sub="Every command, file change and test as it happens, with the diff and the cost."
        t={t}
        title={['Watch it', '*work.*']}
        titleScale={2.6}
        width={520}
        x={110}
        y={360}
      />
      <Caption
        at={T.behind + 0.25}
        exitAt={T.exit - 0.1}
        exitTo={{ x: -90 }}
        sub="It reports back in the task’s thread, and the task moves to review."
        t={t}
        title={['Get a pull', 'request *back.*']}
        titleScale={2.6}
        width={560}
        x={110}
        y={340}
      />
      <div className="tr-abs" ref={m.root} style={windowStyle}>
        <AppWindow height={WIN.h} width={WIN.w}>
          <Shell
            sidebar={
              <ProjectSidebar
                active={t >= T.page ? 'runs' : 'tasks'}
                agents={[
                  agentChange(t, T.starting + 0.15, agents.codex, { state: 'idle', status: 'Ready for work' }, { state: 'working', status: 'Working on NW-12' }),
                  { actor: agents.claude, state: 'working', status: 'Starting NW-16' },
                  { actor: agents.muse, state: 'working', status: 'Mocking the Export button' },
                ]}
                channels={SIDEBAR_CHANNELS}
              />
            }
          >
            {taskShown ? (
              <div className={onRunPage ? 'tr-leaving' : undefined}>
                <div style={onRunPage ? { opacity: clamp(1 - change * 1.6), transform: `translate3d(${-SLIDE * change}px, 0, 0)` } : undefined}>
                <TaskPage
                  reveal={() => 1}
                  start={
                    <StartCard
                      latest={launched ? <LatestLaunch change={{ from: 'starting', k: progress(t, T.working, 0.3) }} state="working" /> : null}
                      latestIn={progress(t, T.starting, 0.35)}
                      pressed={pressed(clicks, t)}
                      readyOut={progress(t, T.working, 0.4)}
                    />
                  }
                  status="progress"
                  statusChange={{ from: 'ready', k: progress(t, T.working, 0.3) }}
                  whiteboard={1}
                />
                </div>
              </div>
            ) : null}
            {onRunPage ? (
              <div style={{ opacity: clamp((change - 0.2) / 0.8), transform: `translate3d(${SLIDE * (1 - change)}px, 0, 0)` }}>
                <RunPage t={t - T.page} />
              </div>
            ) : null}
          </Shell>
        </AppWindow>
        <Cursor clicks={clicks} hideAt={T.leave - 0.25} path={path} scale={1.6} showAt={T.pointer} t={t} />
      </div>
      {t >= T.behind - 0.05 ? <Report t={t - T.behind} /> : null}
    </div>
  )
}

/** Codex's report in the task's thread, and the run card the thread keeps under it. */
function Report({ t }: { t: number }) {
  const k = progress(t, 0, 0.6)
  const out = progress(t, T.exit - T.behind, 0.45, ease.inOut)
  // It keeps coming forward, slowly, until it leaves.
  const creep = 1 + 0.03 * progress(t, 0, T.exit - T.behind + 0.45, ease.linear)
  const style: CSSProperties = {
    left: PANEL.x,
    top: PANEL.y,
    width: PANEL.w,
    height: PANEL.h,
    opacity: k * (1 - out),
    transform: `translate3d(${(PANEL.w * PANEL.k * (1 - creep)) / 2}px, ${(1 - k) * 40 - out * 30 + (PANEL.h * PANEL.k * (1 - creep)) / 2}px, 0) scale(${PANEL.k * mix(0.96, 1, k) * creep})`,
    zIndex: 5,
  }
  const approved = progress(t, R.approved, 0.4)
  return (
    <div className="tr-panel tr-report" style={style}>
      <header className="aw-convhead">
        <div className="aw-convhead__main">
          <h1 className="aw-convhead__heading">
            <span className="aw-convhead__title aw-convhead__title--static">
              <Icon className="aw-convhead__glyph" name="messages" size={18} />
              <span className="aw-convhead__name">Export tasks as CSV</span>
              <Icon className="aw-convhead__caret" name="chevron-down" size={14} />
            </span>
          </h1>
          <div className="aw-convhead__topic">
            <span className="aw-convhead__crumb">in #launch</span>
          </div>
        </div>
      </header>
      <Timeline pinned={false}>
        <Arrive id="runs-report" k={progress(t, R.message, 0.45)}>
          <Message
            entry={REPORT}
            extras={<Reactions id="runs-report" k={progress(t, R.cheer, 0.3)} reactions={[reaction('🎉', 'tada', [people.priya], true)]} />}
          />
        </Arrive>
        <Arrive id="runs-card" k={progress(t, R.card, 0.45)}>
          <div className="aw-timeline__card">
            <RunCard approved={approved} />
          </div>
        </Arrive>
      </Timeline>
    </div>
  )
}

/*
 * The thread's run card, in the app's own markup (threads/ThreadPresentation's
 * RunCard). It is drawn here rather than mounted so Codex keeps the avatar
 * color it has everywhere else in the trailer.
 */
type Chip = { label: string; tone: 'agent' | 'success' | 'muted'; icon?: 'check' | 'clock' }
const WAITING: Chip[] = [{ label: 'Waiting for review', tone: 'muted', icon: 'clock' }]
const APPROVED: Chip[] = [
  { label: 'Approved', tone: 'success', icon: 'check' },
  { label: 'Ready to merge', tone: 'success' },
]

function Chips({ chips }: { chips: Chip[] }) {
  return (
    <span className="tr-chips">
      {chips.map((chip) => (
        <StatusChip icon={chip.icon} key={chip.label} tone={chip.tone}>
          {chip.label}
        </StatusChip>
      ))}
    </span>
  )
}

/** The review landing: the waiting chip gives way to approved and ready to merge. */
function RunCard({ approved }: { approved: number }) {
  return (
    <section aria-label="Run done" className="aw-card aw-run">
      <header className="aw-card__head">
        <Avatar kind="agent" name={agents.codex.name} seed={agents.codex.id} size={24} />
        <div className="aw-card__titles">
          <span className="aw-card__title">Run by {agents.codex.name}</span>
          <span className="aw-card__sub">Updated just now</span>
        </div>
        <StatusChip icon="circle-check" tone="success">
          Done
        </StatusChip>
      </header>
      <div className="aw-run__row">
        <Icon name="branch" size={14} />
        <code>{RUN_BRANCH}</code>
        <span className="aw-run__meta">published 3f9c2a1</span>
      </div>
      <div className="aw-run__row">
        <Icon name="pull-request" size={14} />
        <a href="#pr" onClick={(event) => event.preventDefault()}>
          #{RUN_PR.number} {RUN_PR.title}
        </a>
        <span className="aw-run__checks is-success">
          <Icon name="circle-check" size={14} />
          Checks passed
        </span>
      </div>
      <div aria-label="Pull request" className="aw-run__chips" role="group">
        <StatusChip tone="agent">Open</StatusChip>
        <Swap from={<Chips chips={WAITING} />} id="run-card-review" inline k={approved} to={<Chips chips={APPROVED} />} />
      </div>
      <div className="aw-run__row">
        <Icon name="activity" size={14} />
        <a href="#run" onClick={(event) => event.preventDefault()}>
          See every step, the diff and the cost
        </a>
      </div>
      <footer className="aw-card__actions">
        <Button icon="retry" size="sm">
          Run again
        </Button>
      </footer>
    </section>
  )
}
