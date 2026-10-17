import { useMemo, useState, type KeyboardEvent } from 'react'
import { Link, useNavigate } from 'react-router-dom'

import { useMilestoneList } from '../work-items/planningHooks'
import { Banner, Button, Checkbox, EmptyState, ErrorState, Icon, PageHeader, PageSkeleton, SegmentedControl, SelectField, StatusChip } from '../ui'
import type { Graph, GraphEdge, GraphNode } from './api'
import { PlanningFrame } from './frame'
import { NODE_HEIGHT, NODE_WIDTH, edgePath, layoutGraph } from './graphLayout'
import { useDecideSuggestion, useGraph } from './hooks'
import { aiErrorMessage, planningHref } from './links'

type View = 'graph' | 'list'

const narrow = () => typeof window !== 'undefined' && typeof window.matchMedia === 'function' && window.matchMedia('(max-width: 640px)').matches

const shorten = (text: string, length: number) => (text.length > length ? `${text.slice(0, length - 1)}…` : text)

/** V7: what blocks what, what can start now, and the longest chain of open work. */
export function GraphPage() {
  return (
    <PlanningFrame label="Loading the graph" tab="graph" wide>
      {(project) => <GraphBody projectId={project.id} />}
    </PlanningFrame>
  )
}

function GraphBody({ projectId }: { projectId: string }) {
  const [view, setView] = useState<View>(() => (narrow() ? 'list' : 'graph'))
  const [milestoneId, setMilestoneId] = useState('')
  const [includeFinished, setIncludeFinished] = useState(false)
  const [selected, setSelected] = useState<string | null>(null)
  const milestones = useMilestoneList(projectId)
  const graph = useGraph(projectId, { milestoneId: milestoneId || null, includeFinished })
  const suggested = graph.data?.edges.filter((edge) => edge.suggestionId) ?? []

  return (
    <>
      <PageHeader icon="branch" title="Dependency graph">
        Each line runs from a task to the task it blocks. Tasks on the left can start first; the highlighted chain is the longest run of open
        work. Dashed lines are the AI’s suggestions.
      </PageHeader>
      <div className="aw-ai-toolbar">
        <SegmentedControl<View>
          label="Show as"
          onChange={setView}
          options={[
            { value: 'graph', label: 'Graph', icon: 'branch' },
            { value: 'list', label: 'List', icon: 'list' },
          ]}
          value={view}
        />
        {milestones.data?.milestones.length ? (
          <SelectField className="aw-ai-toolbar__select" label="Milestone" onChange={(event) => setMilestoneId(event.target.value)} value={milestoneId}>
            <option value="">All milestones</option>
            {milestones.data.milestones.map((milestone) => (
              <option key={milestone.id} value={milestone.id}>
                {milestone.name}
              </option>
            ))}
          </SelectField>
        ) : null}
        <Checkbox checked={includeFinished} label="Show finished tasks" onChange={setIncludeFinished} />
      </div>
      {graph.isPending ? <PageSkeleton cards={2} label="Loading the graph" /> : null}
      {graph.isError ? (
        <ErrorState action={<Button onClick={() => void graph.refetch()}>Try again</Button>} title="The graph didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : null}
      {graph.isSuccess && !graph.data.nodes.length ? (
        <EmptyState icon="branch" title="No dependencies to show">
          Tasks show up here once one blocks another. Add “blocked by” on a task, or let the AI suggest some from{' '}
          <Link to={planningHref(projectId, 'suggestions')}>Suggestions</Link>.
        </EmptyState>
      ) : null}
      {graph.data?.truncated ? (
        <Banner role="note" tone="info">
          This project has more open tasks than the graph can show at once. Pick a milestone to see all of its tasks.
        </Banner>
      ) : null}
      {graph.isSuccess && graph.data.nodes.length ? (
        view === 'graph' ? (
          <GraphCanvas graph={graph.data} onSelectEdge={setSelected} selected={selected} />
        ) : (
          <GraphList graph={graph.data} />
        )
      ) : null}
      {graph.isSuccess ? <Legend /> : null}
      {suggested.length ? (
        <SuggestedEdges canEdit={graph.data?.canEdit ?? false} edges={suggested} graph={graph.data!} projectId={projectId} selected={selected} />
      ) : null}
    </>
  )
}

function Legend() {
  return (
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
  )
}

function GraphCanvas({ graph, onSelectEdge, selected }: { graph: Graph; onSelectEdge: (id: string | null) => void; selected: string | null }) {
  const navigate = useNavigate()
  const layout = useMemo(() => layoutGraph(graph), [graph])
  const byId = useMemo(() => new Map(graph.nodes.map((node) => [node.id, node])), [graph])
  const critical = useMemo(() => {
    const pairs = new Set<string>()
    graph.criticalPath.forEach((id, index) => {
      const next = graph.criticalPath[index + 1]
      if (next) pairs.add(`${id}>${next}`)
    })
    return pairs
  }, [graph])
  const open = (node: GraphNode) => navigate(node.href)
  const onKeyDown = (event: KeyboardEvent<SVGGElement>, node: GraphNode) => {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault()
      open(node)
    }
  }

  return (
    <div className="aw-ai-graph" tabIndex={-1}>
      <svg aria-label={`Dependency graph of ${graph.nodes.length} tasks`} height={layout.height} role="group" viewBox={`0 0 ${layout.width} ${layout.height}`} width={layout.width}>
        <defs>
          <marker id="aw-ai-arrow" markerHeight="8" markerWidth="8" orient="auto-start-reverse" refX="8" refY="4" viewBox="0 0 8 8">
            <path d="M 0 0 L 8 4 L 0 8 z" fill="context-stroke" />
          </marker>
        </defs>
        {graph.edges.map((edge) => {
          const from = layout.placed.get(edge.from)
          const to = layout.placed.get(edge.to)
          if (!from || !to) return null
          const classes = [
            'aw-ai-edge',
            edge.suggestionId ? 'aw-ai-edge--suggested' : '',
            critical.has(`${edge.from}>${edge.to}`) ? 'aw-ai-edge--critical' : '',
            edge.suggestionId && edge.suggestionId === selected ? 'is-selected' : '',
          ]
            .filter(Boolean)
            .join(' ')
          return (
            <path
              className={classes}
              d={edgePath(from, to)}
              key={`${edge.from}>${edge.to}`}
              markerEnd="url(#aw-ai-arrow)"
              onClick={edge.suggestionId ? () => onSelectEdge(edge.suggestionId) : undefined}
            >
              <title>
                {`${byId.get(edge.from)?.key ?? ''} blocks ${byId.get(edge.to)?.key ?? ''}${edge.reason ? `: ${edge.reason}` : ''}`}
              </title>
            </path>
          )
        })}
        {graph.nodes.map((node) => {
          const place = layout.placed.get(node.id)
          if (!place) return null
          const classes = [
            'aw-ai-node',
            node.ready && !node.finished ? 'aw-ai-node--ready' : '',
            node.critical ? 'aw-ai-node--critical' : '',
            node.finished ? 'aw-ai-node--finished' : '',
            node.external ? 'aw-ai-node--external' : '',
          ]
            .filter(Boolean)
            .join(' ')
          return (
            <g
              aria-label={`${node.key} ${node.title}, ${node.statusName}${node.ready && !node.finished ? ', can start now' : ''}`}
              className={classes}
              key={node.id}
              onClick={() => open(node)}
              onKeyDown={(event) => onKeyDown(event, node)}
              role="link"
              tabIndex={0}
              transform={`translate(${place.x} ${place.y})`}
            >
              <rect height={NODE_HEIGHT} rx="8" width={NODE_WIDTH} />
              <text className="aw-ai-node__key" x="12" y="21">
                {node.key}
                {node.estimate ? ` · ${node.estimate}` : ''}
              </text>
              <text className="aw-ai-node__status" textAnchor="end" x={NODE_WIDTH - 12} y="21">
                {shorten(node.statusName, 14)}
              </text>
              <text className="aw-ai-node__title" x="12" y="42">
                {shorten(node.title, 28)}
              </text>
              <title>{node.title}</title>
            </g>
          )
        })}
      </svg>
    </div>
  )
}

/** The same graph as a list: easier on a phone and with a screen reader. */
function GraphList({ graph }: { graph: Graph }) {
  const byId = new Map(graph.nodes.map((node) => [node.id, node]))
  const blockedBy = (id: string) => graph.edges.filter((edge) => edge.to === id && !edge.suggestionId).map((edge) => byId.get(edge.from))
  const blocks = (id: string) => graph.edges.filter((edge) => edge.from === id && !edge.suggestionId).map((edge) => byId.get(edge.to))
  const frontier = new Set(graph.frontier)
  const order = [...graph.frontier.map((id) => byId.get(id)), ...graph.nodes.filter((node) => !frontier.has(node.id))].filter(
    (node): node is GraphNode => Boolean(node),
  )
  const names = (nodes: (GraphNode | undefined)[]) =>
    nodes
      .filter((node): node is GraphNode => Boolean(node))
      .map((node, index) => (
        <span key={node.id}>
          {index ? ', ' : ''}
          <Link to={node.href}>{node.key}</Link>
        </span>
      ))
  return (
    <ul aria-label="Tasks and what blocks them" className="aw-ai-graphlist">
      {order.map((node) => (
        <li className={node.finished ? 'is-finished' : undefined} key={node.id}>
          <Link className="aw-ai-rank__title" to={node.href}>
            <span className="aw-ai-task__key">{node.key}</span> {node.title}
          </Link>
          <div className="aw-ai-rank__meta">
            <StatusChip>{node.statusName}</StatusChip>
            {node.ready && !node.finished ? <StatusChip icon="circle-check" tone="success">Can start now</StatusChip> : null}
            {node.critical ? <StatusChip icon="activity" tone="warning">Longest chain</StatusChip> : null}
            {node.external ? <StatusChip icon="link">Another project</StatusChip> : null}
          </div>
          {blockedBy(node.id).length ? <p className="aw-ai-graphlist__line">Blocked by {names(blockedBy(node.id))}</p> : null}
          {blocks(node.id).length ? <p className="aw-ai-graphlist__line">Blocks {names(blocks(node.id))}</p> : null}
        </li>
      ))}
    </ul>
  )
}

function SuggestedEdges({
  canEdit,
  edges,
  graph,
  projectId,
  selected,
}: {
  canEdit: boolean
  edges: GraphEdge[]
  graph: Graph
  projectId: string
  selected: string | null
}) {
  const decide = useDecideSuggestion(projectId)
  const [problem, setProblem] = useState<string | null>(null)
  const byId = new Map(graph.nodes.map((node) => [node.id, node]))
  return (
    <section aria-labelledby="ai-suggested-edges" className="aw-ai-suggested">
      <h2 className="aw-worksection__title" id="ai-suggested-edges">
        <Icon name="bot" size={16} />
        Suggested dependencies
      </h2>
      {problem ? <Banner tone="danger">{problem}</Banner> : null}
      <ul className="aw-ai-suggestions">
        {edges.map((edge) => {
          const from = byId.get(edge.from)
          const to = byId.get(edge.to)
          const id = edge.suggestionId ?? ''
          return (
            <li className={['aw-ai-suggestion', id === selected ? 'is-selected' : ''].filter(Boolean).join(' ')} key={id}>
              <div className="aw-ai-suggestion__body">
                <p className="aw-ai-suggestion__what">
                  {from ? <Link to={from.href}>{from.key}</Link> : 'A task'} blocks {to ? <Link to={to.href}>{to.key}</Link> : 'a task'}
                  {edge.confidence != null ? <span className="aw-ai-suggestion__when"> · {Math.round(edge.confidence * 100)}% sure</span> : null}
                </p>
                {edge.reason ? <p className="aw-ai-suggestion__why">{edge.reason}</p> : null}
              </div>
              {canEdit ? (
                <div className="aw-ai-suggestion__actions">
                  <Button
                    busy={decide.isPending && decide.variables?.suggestionId === id && decide.variables.decision === 'accept'}
                    icon="check"
                    onClick={() => decide.mutate({ suggestionId: id, decision: 'accept' }, { onError: (error) => setProblem(aiErrorMessage(error, 'Accepting')) })}
                    size="sm"
                    variant="primary"
                  >
                    Accept
                  </Button>
                  <Button
                    disabled={decide.isPending}
                    onClick={() => decide.mutate({ suggestionId: id, decision: 'dismiss' }, { onError: (error) => setProblem(aiErrorMessage(error, 'Dismissing')) })}
                    size="sm"
                    variant="ghost"
                  >
                    Dismiss
                  </Button>
                </div>
              ) : null}
            </li>
          )
        })}
      </ul>
    </section>
  )
}
