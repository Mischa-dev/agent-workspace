import type { CSSProperties } from 'react'

import { NODE_HEIGHT, NODE_WIDTH, edgePath, layoutGraph } from '../../../src/ai/graphLayout'
import { Button, Checkbox, Icon, PageHeader, PageTabs, SegmentedControl, SelectField } from '../../../src/ui'
import { agents, project, tasks } from '../data'
import { clamp, ease, mix, progress } from '../motion'
import { Cursor, type Waypoint } from '../parts/Cursor'
import { useBoxes } from '../parts/measure'
import { AppWindow, ProjectSidebar, Shell } from '../parts/Shell'
import { Caption } from '../parts/Text'

/*
 * 0:28. Planning: the dependency graph, laid out by the app's own graph code.
 * What can start now is ringed, the longest chain to launch lights up, and a
 * suggested dependency gets accepted.
 */

const WIN = { w: 1580, h: 960, x: 64, y: 110, k: 0.8 }

const T = {
  nodes: 0.35,
  ready: 1.7,
  critical: 2.2,
  suggest: 2.7,
  accept: 3.95,
  exit: 5.9,
}

type Node = { n: number; status: string; title?: string; key?: string; finished?: boolean }

const NODES: Node[] = [
  { n: 9, status: 'Done', finished: true },
  { n: 14, status: 'In progress' },
  { n: 16, status: 'Backlog' },
  { n: 18, status: 'In progress' },
  { n: 19, status: 'In review' },
  { n: 12, status: 'Ready' },
  { n: 27, status: 'Backlog' },
  { n: 24, status: 'Ready' },
  { n: 21, status: 'Backlog' },
  { n: 30, status: 'Backlog', title: 'Ship 1.4' },
]

const EDGES: [number, number][] = [
  [9, 12],
  [14, 27],
  [16, 24],
  [12, 21],
  [27, 21],
  [21, 30],
  [24, 30],
  [18, 30],
  [19, 30],
]

const SUGGESTED: [number, number] = [24, 21]
// The longest chain to launch lights up one link at a time, from its first task to Ship 1.4.
const CRITICAL = ['14>27', '27>21', '21>30']
const CRITICAL_NODES = [14, 27, 21, 30]
const LINK = 0.22
const READY = [12, 16]

const id = (n: number) => `n${n}`
const layout = layoutGraph({
  nodes: NODES.map((node) => ({ id: id(node.n) })),
  edges: EDGES.map(([from, to]) => ({ from: id(from), to: id(to) })),
})

const shorten = (text: string, length: number) => (text.length > length ? `${text.slice(0, length - 1)}…` : text)

export function Graph({ t }: { t: number }) {
  const m = useBoxes('graph', { accept: '.tr-accept' })
  const arrive = progress(t, 0, 0.55)
  const exit = progress(t, T.exit, 0.45, ease.inOut)
  // A slow push toward the graph the whole time it's on screen.
  const push = 1 + 0.03 * progress(t, 0, T.exit + 0.45, ease.linear)
  const windowStyle: CSSProperties = {
    left: 0,
    top: 0,
    width: WIN.w,
    height: WIN.h,
    transformOrigin: '0 0',
    opacity: arrive * (1 - exit),
    transform: `translate3d(${WIN.x + (1 - arrive) * 60 - exit * 80}px, ${WIN.y}px, 0) scale(${WIN.k * mix(0.97, 1, arrive)})`,
  }
  const accepted = t >= T.accept + 0.1
  const path: Waypoint[] = [
    { t: 2.9, x: 1250, y: 900 },
    { t: T.accept - 0.05, ...m.center('accept', -10, 2) },
    { t: T.accept + 0.6, ...m.center('accept', 60, 60) },
  ]

  return (
    <div className="tr-layer">
      <Caption
        at={0.4}
        exitAt={T.exit}
        exitTo={{ x: -90 }}
        kicker="Planning"
        kickerIcon={<Icon name="branch" size={26} />}
        sub="What blocks what, the longest chain to launch, and the links you might have missed."
        t={t}
        title={['See what’s', 'ready to start.']}
        titleScale={2.5}
        width={470}
        x={1390}
        y={330}
      />
      <div className="tr-layer" style={{ transform: `scale(${push})`, transformOrigin: '760px 560px' }}>
      <div className="tr-abs" ref={m.root} style={windowStyle}>
        <AppWindow height={WIN.h} width={WIN.w}>
          <Shell
            sidebar={
              <ProjectSidebar
                active="tasks"
                agents={[
                  { actor: agents.codex, state: 'idle', status: 'Ready for work' },
                  { actor: agents.claude, state: 'working', status: 'Fixing NW-18' },
                  { actor: agents.muse, state: 'working', status: 'Designing NW-14' },
                ]}
                channels={[{ name: 'general' }, { name: 'launch', unread: true }, { name: 'design' }]}
              />
            }
          >
            <div className="aw-page aw-page--full aw-ai-page">
              <PageHeader icon="branch" title="Planning">
                What can start next in {project.name}, what depends on what, and plans to turn into tasks.
              </PageHeader>
              <PageTabs
                current="planning"
                label="Task views"
                tabs={[
                  { id: 'tasks', label: 'Tasks', to: '#', icon: 'work' },
                  { id: 'milestones', label: 'Milestones', to: '#', icon: 'checklist' },
                  { id: 'timeline', label: 'Timeline', to: '#', icon: 'clock' },
                  { id: 'planning', label: 'Planning', to: '#', icon: 'branch' },
                ]}
              />
              <nav aria-label="Planning" className="aw-seg aw-ai-subnav">
                {(
                  [
                    ['next', 'Up next', 'play'],
                    ['suggestions', 'Suggestions', 'bot'],
                    ['graph', 'Graph', 'branch'],
                    ['plan-import', 'Import a plan', 'list'],
                  ] as const
                ).map(([key, label, icon]) => (
                  <span aria-current={key === 'graph' ? 'page' : undefined} className="aw-seg__opt" key={key}>
                    <Icon name={icon} size={14} />
                    {label}
                    {key === 'suggestions' && !accepted ? <span className="aw-ai-subnav__count">1</span> : null}
                  </span>
                ))}
              </nav>
              <div className="aw-ai-toolbar">
                <SegmentedControl
                  label="Show as"
                  onChange={() => undefined}
                  options={[
                    { value: 'graph', label: 'Graph', icon: 'branch' },
                    { value: 'list', label: 'List', icon: 'list' },
                  ]}
                  value="graph"
                />
                <SelectField aria-label="Milestone" className="aw-ai-toolbar__select" compact onChange={() => undefined} value="m">
                  <option value="m">1.4 launch</option>
                </SelectField>
                <Checkbox checked={false} label="Show finished tasks" onChange={() => undefined} />
              </div>
              <GraphCanvas t={t} />
              <ul aria-label="Legend" className="aw-ai-legend">
                <li>
                  <span className="aw-ai-legend__swatch aw-ai-legend__swatch--ready" /> Can start now
                </li>
                <li>
                  <span className="aw-ai-legend__swatch aw-ai-legend__swatch--critical" /> Longest chain
                </li>
                <li>
                  <span className="aw-ai-legend__line aw-ai-legend__line--suggested" /> Suggested
                </li>
                <li>
                  <span className="aw-ai-legend__swatch aw-ai-legend__swatch--external" /> Another project
                </li>
              </ul>
              <Suggestion accepted={accepted} pressed={t >= T.accept && t < T.accept + 0.16} t={t} />
            </div>
          </Shell>
        </AppWindow>
        <Cursor clicks={[T.accept]} hideAt={T.accept + 0.7} path={path} scale={1.6} showAt={2.9} t={t} />
      </div>
      </div>
    </div>
  )
}

function GraphCanvas({ t }: { t: number }) {
  const placed = (n: number) => layout.placed.get(id(n))
  const nodeIn = (n: number) => {
    const place = placed(n)
    return progress(t, T.nodes + (place?.column ?? 0) * 0.28 + (place?.row ?? 0) * 0.05, 0.45)
  }
  const suggestIn = progress(t, T.suggest, 0.5)
  // Accepting turns the dashed suggestion into a link like the others.
  const accept = progress(t, T.accept + 0.1, 0.4, ease.inOut)
  const edges = [...EDGES, ...(t >= T.suggest ? [SUGGESTED] : [])]
  const lit = (key: string) => (CRITICAL.includes(key) ? progress(t, T.critical + CRITICAL.indexOf(key) * LINK, 0.3, ease.inOut) : 0)
  const litNode = (n: number) => (CRITICAL_NODES.includes(n) ? progress(t, T.critical + CRITICAL_NODES.indexOf(n) * LINK - 0.08, 0.3) : 0)
  return (
    <div className="aw-ai-graph tr-graph">
      <svg height={layout.height} viewBox={`0 0 ${layout.width} ${layout.height}`} width={layout.width}>
        <defs>
          <marker id="tr-arrow" markerHeight="8" markerWidth="8" orient="auto-start-reverse" refX="8" refY="4" viewBox="0 0 8 8">
            <path d="M 0 0 L 8 4 L 0 8 z" fill="context-stroke" />
          </marker>
        </defs>
        {edges.map(([fromN, toN]) => {
          const from = placed(fromN)
          const to = placed(toN)
          if (!from || !to) return null
          const key = `${fromN}>${toN}`
          const d = edgePath(from, to)
          if (fromN === SUGGESTED[0] && toN === SUGGESTED[1]) {
            return (
              <g key={key}>
                {accept < 1 ? <path className="aw-ai-edge aw-ai-edge--suggested" d={d} style={{ opacity: suggestIn * (1 - accept) }} /> : null}
                {accept > 0 ? (
                  <path className="aw-ai-edge" d={d} markerEnd={accept > 0.95 ? 'url(#tr-arrow)' : undefined} pathLength={1} style={{ strokeDasharray: '1 1', strokeDashoffset: 1 - accept }} />
                ) : null}
              </g>
            )
          }
          const draw = progress(t, T.nodes + (from.column + 1) * 0.28 - 0.08, 0.4, ease.inOut)
          const k = lit(key)
          return (
            <g key={key}>
              <path className="aw-ai-edge" d={d} markerEnd={draw > 0.95 && k < 0.5 ? 'url(#tr-arrow)' : undefined} pathLength={1} style={{ strokeDasharray: '1 1', strokeDashoffset: 1 - draw }} />
              {k > 0 ? (
                <path className="aw-ai-edge aw-ai-edge--critical" d={d} markerEnd={k > 0.5 ? 'url(#tr-arrow)' : undefined} pathLength={1} style={{ strokeDasharray: '1 1', strokeDashoffset: 1 - k }} />
              ) : null}
              {/* Once lit, a glint runs down the chain toward launch, over and over. */}
              {k >= 1 ? (
                <path
                  className="tr-glint"
                  d={d}
                  pathLength={1}
                  style={{ strokeDasharray: '0.12 2', strokeDashoffset: 0.12 - (((t - T.critical - CRITICAL.indexOf(key) * LINK) * 0.9) % 1) * 1.12 }}
                />
              ) : null}
            </g>
          )
        })}
        {NODES.map((node) => {
          const place = placed(node.n)
          if (!place) return null
          const k = nodeIn(node.n)
          const seed = tasks[node.n]
          const title = node.title ?? seed?.title ?? ''
          const ready = READY.includes(node.n) ? progress(t, T.ready + READY.indexOf(node.n) * 0.12, 0.35) : 0
          const ping = READY.includes(node.n) ? clamp((t - T.ready - READY.indexOf(node.n) * 0.12) / 0.7) : 0
          const warm = litNode(node.n)
          return (
            <g
              className={node.finished ? 'aw-ai-node aw-ai-node--finished' : 'aw-ai-node'}
              key={node.n}
              style={{ opacity: (node.finished ? 0.55 : 1) * k }}
              transform={`translate(${place.x} ${place.y + (1 - k) * 10})`}
            >
              <rect height={NODE_HEIGHT} rx="8" width={NODE_WIDTH} />
              {warm > 0 ? <rect height={NODE_HEIGHT} rx="8" style={{ fill: 'var(--warning-soft)', stroke: 'none', opacity: warm }} width={NODE_WIDTH} /> : null}
              {ready > 0 ? <rect height={NODE_HEIGHT} rx="8" style={{ fill: 'none', stroke: 'var(--success)', strokeWidth: 2, opacity: ready }} width={NODE_WIDTH} /> : null}
              {ping > 0 && ping < 1 ? (
                <rect
                  height={NODE_HEIGHT + 16 * ease.out(ping)}
                  rx={8 + 8 * ease.out(ping)}
                  style={{ fill: 'none', stroke: 'var(--success)', strokeWidth: 2, opacity: 0.7 * (1 - ping) }}
                  width={NODE_WIDTH + 16 * ease.out(ping)}
                  x={-8 * ease.out(ping)}
                  y={-8 * ease.out(ping)}
                />
              ) : null}
              <text className="aw-ai-node__key" x="12" y="21">
                {`${project.key}-${node.n}`}
              </text>
              <text className="aw-ai-node__status" textAnchor="end" x={NODE_WIDTH - 12} y="21">
                {node.status}
              </text>
              <text className="aw-ai-node__title" x="12" y="42">
                {shorten(title, 28)}
              </text>
            </g>
          )
        })}
      </svg>
    </div>
  )
}

function Suggestion({ accepted, pressed, t }: { accepted: boolean; pressed: boolean; t: number }) {
  const k = progress(t, T.suggest + 0.2, 0.5)
  const gone = progress(t, T.accept + 0.15, 0.35, ease.inOut)
  if (gone >= 1) return null
  return (
    <section className="aw-ai-suggested" style={{ opacity: k * (1 - gone), transform: `translate3d(0, ${(1 - k) * 12}px, 0)` }}>
      <h2 className="aw-worksection__title">
        <Icon name="bot" size={16} />
        Suggested dependencies
      </h2>
      <ul className="aw-ai-suggestions">
        <li className="aw-ai-suggestion is-selected">
          <div className="aw-ai-suggestion__body">
            <p className="aw-ai-suggestion__what">
              <span className="tr-link">NW-24</span> blocks <span className="tr-link">NW-21</span>
              <span className="aw-ai-suggestion__when"> · 82% sure</span>
            </p>
            <p className="aw-ai-suggestion__why">The export docs should say how failed deliveries are retried.</p>
          </div>
          <div className="aw-ai-suggestion__actions">
            <Button className={pressed ? 'tr-accept tr-pressed' : 'tr-accept'} icon="check" size="sm" variant="primary">
              {accepted ? 'Accepted' : 'Accept'}
            </Button>
            <Button size="sm" variant="ghost">
              Dismiss
            </Button>
          </div>
        </li>
      </ul>
    </section>
  )
}
