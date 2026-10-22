import { swatchFor, type Swatch } from '../../src/ui'
import type { ConversationActor, ConversationMessage, ConversationTimelineEntry, ConversationWorkItemReference } from '../../src/conversations/types'
import type { WorkItemCard, WorkItemLabel, WorkItemPriority } from '../../src/work-items/types'

/*
 * The demo workspace the trailer shows: made-up people, agents from several
 * makers, and one project. Nothing here comes from a real workspace.
 */

/** A UUID whose swatch is the one asked for, so everyone keeps a chosen color. */
function idFor(index: number, swatch: Swatch) {
  for (let n = 0; n < 4096; n += 1) {
    const id = `00000000-0000-4000-8000-${(index * 4096 + n).toString(16).padStart(12, '0')}`
    if (swatchFor(id) === swatch) return id
  }
  throw new Error(`No id for ${swatch}`)
}

export type Actor = {
  id: string
  name: string
  handle: string
  kind: 'person' | 'agent'
  swatch: Swatch
  /** What an agent is, shown on the roster. */
  made?: string
}

function person(index: number, name: string, swatch: Swatch): Actor {
  return { id: idFor(index, swatch), name, handle: name.split(' ')[0].toLowerCase(), kind: 'person', swatch }
}

function agent(index: number, name: string, swatch: Swatch, made: string): Actor {
  return { id: idFor(index, swatch), name, handle: name.toLowerCase().replace(/\s+/g, ''), kind: 'agent', swatch, made }
}

export const people = {
  priya: person(1, 'Priya Shah', 'blue'),
  leo: person(2, 'Leo Martin', 'green'),
  sam: person(3, 'Sam Okafor', 'rose'),
  maya: person(4, 'Maya Chen', 'indigo'),
}

export const agents = {
  claude: agent(10, 'Claude', 'rust', 'Claude Code'),
  codex: agent(11, 'Codex', 'slate', 'Codex'),
  chatgpt: agent(12, 'ChatGPT', 'green', 'ChatGPT agent'),
  grok: agent(13, 'Grok', 'indigo', 'Grok'),
  muse: agent(14, 'Muse', 'orchid', 'Meta Muse'),
  hermes: agent(15, 'Hermes', 'teal', 'Hermes'),
  openclaw: agent(16, 'OpenClaw', 'rose', 'OpenClaw'),
  scout: agent(17, 'Scout', 'blue', 'Your own harness'),
}

/** The agent that comes with the workspace (shown as coming next), apart from the demo team's own. */
export const builtinAgent = agent(18, 'Workspace agent', 'indigo', 'Built in')

export const viewer = people.priya

export function conversationActor(actor: Actor): ConversationActor {
  return {
    id: actor.id,
    displayName: actor.name,
    handle: actor.handle,
    kind: actor.kind === 'agent' ? 'external_agent' : 'human',
    avatarUrl: null,
  } as ConversationActor
}

export const project = { id: idFor(30, 'rust'), key: 'NW', name: 'Northwind', swatch: 'rust' as Swatch }

/* Labels carry a swatch token as their color, as the app's label picker does. */
const label = (id: number, name: string, swatch: Swatch): WorkItemLabel => ({
  id: idFor(40 + id, 'slate'),
  name,
  color: `var(--swatch-${swatch})`,
})
export const labels = {
  export: label(1, 'export', 'teal'),
  design: label(2, 'design', 'orchid'),
  api: label(3, 'api', 'blue'),
  mobile: label(4, 'mobile', 'green'),
  docs: label(5, 'docs', 'slate'),
  bug: label(6, 'bug', 'rose'),
  growth: label(7, 'growth', 'rust'),
}

export type StatusKey = 'backlog' | 'ready' | 'progress' | 'review' | 'done'
export const statuses: Record<StatusKey, { id: string; name: string; semanticCategory: string }> = {
  backlog: { id: idFor(50, 'slate'), name: 'Backlog', semanticCategory: 'backlog' },
  ready: { id: idFor(51, 'slate'), name: 'Ready', semanticCategory: 'unstarted' },
  progress: { id: idFor(52, 'slate'), name: 'In progress', semanticCategory: 'started' },
  review: { id: idFor(53, 'slate'), name: 'In review', semanticCategory: 'review' },
  done: { id: idFor(54, 'slate'), name: 'Done', semanticCategory: 'completed' },
}

export const milestone = { id: idFor(60, 'slate'), name: '1.4 launch', targetDate: '2026-11-14' }

type TaskSeed = {
  n: number
  title: string
  status: StatusKey
  priority: WorkItemPriority
  who?: Actor
  labels?: WorkItemLabel[]
  subtasks?: [number, number]
  blocked?: number
  due?: string
}

const seeds: TaskSeed[] = [
  { n: 12, title: 'Export tasks as CSV', status: 'ready', priority: 'high', who: agents.codex, labels: [labels.export], subtasks: [0, 3], due: '2026-11-06' },
  { n: 9, title: 'Agree the column order', status: 'done', priority: 'medium', who: people.leo, labels: [labels.export] },
  { n: 14, title: 'Design the Export button', status: 'progress', priority: 'medium', who: agents.muse, labels: [labels.design] },
  { n: 15, title: 'Bulk edit from the board', status: 'ready', priority: 'medium', who: people.sam, labels: [labels.growth] },
  { n: 16, title: 'Webhooks for task events', status: 'backlog', priority: 'low', who: agents.claude, labels: [labels.api] },
  { n: 17, title: 'Offline drafts on phones', status: 'backlog', priority: 'medium', labels: [labels.mobile], blocked: 1 },
  { n: 18, title: 'Fix stale unread badges', status: 'progress', priority: 'urgent', who: agents.claude, labels: [labels.bug] },
  { n: 19, title: 'Import plans from Markdown', status: 'review', priority: 'high', who: agents.chatgpt, labels: [labels.growth], subtasks: [4, 4] },
  { n: 20, title: 'Usage page for each project', status: 'review', priority: 'medium', who: people.maya, labels: [labels.api] },
  { n: 21, title: 'Write the export docs', status: 'backlog', priority: 'low', who: agents.grok, labels: [labels.docs], blocked: 1 },
  { n: 22, title: 'Faster board on big projects', status: 'done', priority: 'high', who: agents.codex, labels: [labels.growth] },
  { n: 23, title: 'Keyboard moves on the board', status: 'done', priority: 'low', who: people.priya, labels: [labels.design] },
  { n: 24, title: 'Retry failed webhook deliveries', status: 'ready', priority: 'high', who: agents.hermes, labels: [labels.api] },
  { n: 25, title: 'Reminders on any message', status: 'progress', priority: 'medium', who: people.leo, labels: [labels.mobile] },
  { n: 26, title: 'Write the CSV encoder', status: 'ready', priority: 'high', who: agents.codex, labels: [labels.export] },
  { n: 27, title: 'Add an Export button', status: 'ready', priority: 'medium', who: agents.muse, labels: [labels.export, labels.design] },
  { n: 28, title: 'Document the format', status: 'ready', priority: 'low', who: agents.grok, labels: [labels.docs] },
]

export function card(seed: TaskSeed, status: StatusKey = seed.status, overrides: Partial<WorkItemCard> = {}): WorkItemCard {
  const id = idFor(100 + seed.n, 'slate')
  return {
    id,
    projectId: project.id,
    key: `${project.key}-${seed.n}`,
    number: seed.n,
    title: seed.title,
    itemType: { id: 'task', name: 'Task' },
    status: statuses[status],
    priority: seed.priority,
    assignee: seed.who
      ? { actorId: seed.who.id, displayName: seed.who.name, handle: seed.who.handle, kind: seed.who.kind === 'agent' ? 'external_agent' : 'human', active: true }
      : null,
    version: 1,
    archivedAt: null,
    createdAt: '2026-10-20T09:00:00Z',
    updatedAt: '2026-10-21T09:00:00Z',
    labels: seed.labels ?? [],
    capabilities: { view: true, edit: false, archive: false },
    milestone,
    startDate: null,
    dueDate: seed.due ?? null,
    estimate: null,
    ready: status === 'ready',
    parentId: null,
    blockedReason: null,
    openBlockerCount: seed.blocked ?? 0,
    subtaskCount: seed.subtasks?.[1] ?? 0,
    subtasksDone: seed.subtasks?.[0] ?? 0,
    claimedBy: null,
    boardOrder: String(seed.n).padStart(4, '0'),
    ...overrides,
  }
}

export const tasks = Object.fromEntries(seeds.map((seed) => [seed.n, seed])) as Record<number, TaskSeed>
export const allTasks = seeds

export function taskRef(n: number, statusCategory = 'started'): ConversationWorkItemReference {
  const seed = tasks[n]
  return {
    id: idFor(100 + n, 'slate'),
    key: `${project.key}-${n}`,
    title: seed.title,
    statusCategory,
    href: '#',
    archivedAt: null,
  } as unknown as ConversationWorkItemReference
}

/* Messages: text runs, with @mentions and task chips between them. */
type Part = string | Actor | { task: number; state?: string }

export function message(
  id: number,
  author: Actor,
  time: string,
  parts: Part[],
  extra: Partial<ConversationMessage> = {},
): ConversationTimelineEntry {
  const mentioned = parts.filter((part): part is Actor => typeof part === 'object' && 'id' in part)
  const referenced = parts.filter((part): part is { task: number; state?: string } => typeof part === 'object' && 'task' in part)
  const nodes = parts.map((part) =>
    typeof part === 'string'
      ? { type: 'text' as const, text: part }
      : 'task' in part
        ? { type: 'work_item_ref' as const, workItemId: idFor(100 + part.task, 'slate') }
        : { type: 'mention' as const, actorId: part.id },
  )
  const createdAt = `2026-10-21T${time}:00Z`
  return {
    id: `entry-${id}`,
    sequence: id,
    kind: 'message',
    createdAt,
    workerEvent: null,
    message: {
      id: idFor(200 + id, 'slate'),
      author: conversationActor(author),
      content: { version: 1, nodes } as ConversationMessage['content'],
      plainText: parts.map((part) => (typeof part === 'string' ? part : '')).join(''),
      mentionedActors: mentioned.map(conversationActor),
      referencedWorkItems: referenced.map((part) => taskRef(part.task, part.state)),
      createdAt,
      via: null,
      revision: 1,
      editedAt: null,
      removed: null,
      reactions: [],
      ...extra,
    },
  }
}

/** The picture Muse attaches: asset.mjs captures it at 1.5x, and serve.mjs serves it by this id. */
export const museMock = {
  id: '7a1e0000-0000-4000-8000-00000000a001',
  name: 'export-button.png',
  contentType: 'image/png',
  size: 48_210,
  width: 630,
  height: 519,
  commentId: null,
  uploadedBy: agents.muse.id,
  createdAt: '2026-10-21T09:09:00Z',
}
