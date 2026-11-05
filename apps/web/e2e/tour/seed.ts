import { spawnSync } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { mkdirSync, writeFileSync } from 'node:fs'
import path from 'node:path'

import { request, type APIRequestContext } from '@playwright/test'

import {
  agents,
  channels,
  designCritChannel,
  dmLines,
  featuredTask,
  incidentsChannel,
  labels,
  type Line,
  milestones,
  otherPages,
  otherProjects,
  people,
  plan,
  releasesChannel,
  releasesThread,
  tourProject,
  tourTasks,
} from './content'
import { tourStatePath, type TourState } from './state'

// Fills the throwaway server with a busy workspace for the UI tour, through
// the same HTTP API the app uses: people who joined by invitation and post
// as themselves, agents with their own tokens, ~200 tasks, busy channels with
// threads and reactions, a DM, pages and a whiteboard. Two things the API
// can't make go straight into the throwaway database: agent runs (they need a
// repository prepared from GitHub) and spread-out timestamps (everything
// would say "just now"). If the schema has moved on, those steps say so and
// the tour carries on without them.

type Json = Record<string, unknown> & { id?: string }

/** One signed-in person: a cookie session that sends the CSRF token on writes. */
class Session {
  constructor(
    readonly api: APIRequestContext,
    readonly origin: string,
    public actorId = '',
  ) {}

  async get(url: string): Promise<Json> {
    return this.send('GET', url)
  }

  async write(method: 'POST' | 'PATCH' | 'PUT', url: string, data?: unknown): Promise<Json> {
    return this.send(method, url, data)
  }

  private async send(method: string, url: string, data?: unknown): Promise<Json> {
    for (let attempt = 0; ; attempt += 1) {
      const state = await this.api.storageState()
      const csrf = state.cookies.find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
      const response = await this.api.fetch(url, {
        method,
        data,
        headers: { origin: this.origin, 'x-csrf-token': csrf, 'idempotency-key': randomUUID() },
      })
      // 240 writes a minute per session: wait out the limit rather than fail.
      if (response.status() === 429 && attempt < 8) {
        await new Promise((resolve) => setTimeout(resolve, 8_000))
        continue
      }
      if (!response.ok()) throw new Error(`${method} ${url}: HTTP ${response.status()} ${await response.text()}`)
      const text = await response.text()
      return (text ? JSON.parse(text) : {}) as Json
    }
  }
}

/** An agent acting as itself through the public API with its own token. */
class AgentClient {
  constructor(
    readonly api: APIRequestContext,
    readonly token: string,
    readonly actorId: string,
    readonly handle: string,
  ) {}

  async call(tool: string, args: Record<string, unknown>): Promise<Json> {
    const response = await this.api.post(`/api/public/v1/${tool}`, {
      data: args,
      headers: { authorization: `Bearer ${this.token}` },
    })
    if (!response.ok()) throw new Error(`${tool} as ${this.handle}: HTTP ${response.status()} ${await response.text()}`)
    return (await response.json()) as Json
  }
}

type SeedContext = {
  admin: APIRequestContext
  organizationId: string
  serverOrigin: string
  webOrigin: string
  databaseUrl: string
}

const handleOf = (loginName: string) => loginName.replace(/[^a-z0-9_-]/g, '')
const agentHandle = (name: string) => name.toLowerCase().replace(/[^a-z0-9]+/g, '-')

export async function seedTour(context: SeedContext): Promise<TourState> {
  const { serverOrigin, webOrigin } = context
  const me = new Session(context.admin, webOrigin)
  const session = (await me.get('/api/v1/auth/session')) as { user?: { actorId?: string } }
  me.actorId = session.user?.actorId ?? ''
  // Joins the direct messages space, so others can DM them.
  await me.get('/api/v1/direct')
  await me.write('PATCH', '/api/v1/account/profile', { displayName: 'Robin Achterberg' })
  const workspace = (await me.get(`/api/v1/organizations/${context.organizationId}`)) as { version?: number; workspace?: { version?: number } }
  await me.write('PATCH', `/api/v1/organizations/${context.organizationId}`, {
    name: 'Atlas Robotics',
    expectedVersion: Number(workspace.version ?? workspace.workspace?.version ?? 1),
  })

  // Projects: the busy one, and a few others for the rail and Home.
  const project = await me.write('POST', '/api/v1/projects', {
    organizationId: context.organizationId,
    key: tourProject.key,
    name: tourProject.name,
  })
  const projectId = String(project.id)
  const others: string[] = []
  for (const other of otherProjects) {
    const created = await me.write('POST', '/api/v1/projects', { organizationId: context.organizationId, ...other })
    others.push(String(created.id))
  }

  // People join by invitation and sign in as themselves.
  const team: Session[] = []
  for (const person of people) {
    const invitation = await me.write('POST', `/api/v1/projects/${projectId}/invitations`, {
      role: 'member',
      expiresInHours: 24,
    })
    const token = String(invitation.invitationUrl ?? '').split('#token=')[1]
    if (!token) throw new Error('The invitation came back without a token')
    const api = await request.newContext({ baseURL: serverOrigin, extraHTTPHeaders: { origin: webOrigin } })
    const response = await api.post('/api/v1/project-invitations/accept-new', {
      data: {
        token,
        loginName: person.loginName,
        displayName: person.displayName,
        handle: handleOf(person.loginName),
        password: `tour-${randomUUID()}`,
      },
    })
    if (!response.ok()) throw new Error(`accept-new for ${person.loginName}: ${response.status()} ${await response.text()}`)
    const accepted = (await response.json()) as { user?: { actorId?: string } }
    const member = new Session(api, webOrigin, accepted.user?.actorId ?? '')
    // Joins the direct messages space, so DMs can reach them.
    await member.get('/api/v1/direct')
    team.push(member)
  }
  for (const [index, other] of others.entries()) {
    for (const member of team.filter((_, i) => (i + index) % 2 === 0)) {
      await me.write('POST', `/api/v1/projects/${other}/members`, { actorId: member.actorId, role: 'member' })
    }
  }

  // Agents with their own tokens.
  const bots = new Map<string, AgentClient>()
  for (const agent of agents) {
    const handle = agentHandle(agent.name)
    const created = (await me.write('POST', `/api/v1/projects/${projectId}/external-agents`, {
      connectionName: `${agent.name} runner`,
      displayName: agent.name,
      handle,
      role: 'member',
      scopes: ['read', 'write'],
    })) as { connection?: { actorId?: string }; oneTimeCredential?: { token?: string } }
    const api = await request.newContext({ baseURL: serverOrigin })
    bots.set(agent.name, new AgentClient(api, created.oneTimeCredential?.token ?? '', created.connection?.actorId ?? '', handle))
  }

  const author = (who: Line[0]): Session | AgentClient => {
    if (who === 'me') return me
    if (typeof who === 'number') return team[who]
    const bot = bots.get(who)
    if (!bot) throw new Error(`No agent called ${who}`)
    return bot
  }
  const actorIdOf = (login: string) => {
    if (login === 'me') return me.actorId
    const index = people.findIndex((person) => person.loginName === login)
    return index >= 0 ? team[index].actorId : null
  }

  // Labels, milestones and the workflow's statuses.
  const labelIds = new Map<string, string>()
  const palette: Record<string, string> = { rose: 'red', teal: 'teal', blue: 'blue', indigo: 'purple', green: 'green', rust: 'orange', slate: 'gray', orchid: 'pink' }
  for (const label of labels) {
    const created = await me.write('POST', `/api/v1/projects/${projectId}/labels`, { name: label.name, color: palette[label.color] })
    labelIds.set(label.name, String(created.id))
  }
  const milestoneIds = new Map<string, string>()
  for (const [index, name] of milestones.entries()) {
    const created = await me.write('POST', `/api/v1/projects/${projectId}/milestones`, {
      name,
      targetDate: `2026-${String(11 + Math.min(index, 1)).padStart(2, '0')}-${index === 2 ? '20' : '07'}`,
    })
    milestoneIds.set(name, String(created.id))
  }
  const workflow = (await me.get(`/api/v1/projects/${projectId}/work-item-workflow`)) as {
    statuses?: Array<{ id: string; name: string }>
    types?: Array<{ id: string; name: string }>
  }
  const statusId = (name: string) => workflow.statuses?.find((status) => status.name === name)?.id
  const typeId = (name: string) => workflow.types?.find((type) => type.name === name)?.id

  // About 200 tasks, made by the people who'd make them. Every seventh is
  // assigned to the signed-in person by someone else, so the inbox fills.
  const taskIds: string[] = []
  for (const [index, task] of tourTasks().entries()) {
    const creator = team[index % team.length]
    const assignee = index % 7 === 3 ? me.actorId : task.assignee === null ? undefined : team[task.assignee].actorId
    const created = await creator.write('POST', `/api/v1/projects/${projectId}/work-items`, {
      title: task.title,
      description: index % 5 === 0 ? `Raised in #releases. ${task.title}.` : '',
      typeId: typeId(task.labels.includes('bug') ? 'Bug' : index % 9 === 0 ? 'Feature' : 'Task'),
      statusId: statusId(task.status),
      priority: task.priority,
      assigneeActorId: assignee,
      milestoneId: task.milestone ? milestoneIds.get(task.milestone) : undefined,
      labelIds: task.labels.map((name) => labelIds.get(name)).filter(Boolean),
      estimate: index % 4 === 0 ? (index % 8) + 1 : undefined,
      dueDate: index % 11 === 0 ? `2026-11-${String((index % 27) + 1).padStart(2, '0')}` : undefined,
    })
    taskIds.push(String(created.id))
  }
  const featured = await team[3].write('POST', `/api/v1/projects/${projectId}/work-items`, {
    title: featuredTask.title,
    description: featuredTask.description,
    typeId: typeId('Feature'),
    statusId: statusId('In Progress'),
    priority: 'high',
    assigneeActorId: team[3].actorId,
    milestoneId: milestoneIds.get(milestones[0]),
    labelIds: [labelIds.get('dashboard'), labelIds.get('customer-reported')],
    estimate: 5,
    dueDate: '2026-11-07',
    acceptanceCriteria: featuredTask.criteria.map((criterion) => criterion.text),
  })
  const featuredId = String(featured.id)
  const opened = (await me.get(`/api/v1/projects/${projectId}/work-items/${featuredId}`)) as {
    version?: number
    workItem?: { version?: number }
    acceptanceCriteria?: Array<{ id: string; text: string }>
  }
  let version = Number(opened.version ?? opened.workItem?.version ?? 1)
  for (const criterion of opened.acceptanceCriteria ?? []) {
    if (!featuredTask.criteria.find((c) => c.text === criterion.text)?.done) continue
    const changed = await team[3].write(
      'PATCH',
      `/api/v1/projects/${projectId}/work-items/${featuredId}/acceptance-criteria/${criterion.id}`,
      { expectedVersion: version, completed: true },
    )
    version = Number(changed.version ?? changed.workItemVersion ?? version + 1)
  }
  for (const [index, text] of featuredTask.comments.entries()) {
    await team[[2, 0, 3][index]].write('POST', `/api/v1/projects/${projectId}/work-items/${featuredId}/comments`, {
      content: { version: 1, nodes: [{ type: 'text', text }] },
    })
  }
  await bots.get('Scout')!.call('task_comment', {
    taskId: featuredId,
    text: 'Two older tasks asked for the same panel; I linked them here. Support has 14 open conversations that mention cell imbalance this month.',
    clientRequestId: randomUUID(),
  })

  // Text with @login (a mention) and {task} (a link to the featured task) as nodes.
  const nodes = (text: string) => {
    const result: Array<Record<string, string>> = []
    for (const part of text.split(/(@[a-z]+|\{task\})/)) {
      if (!part) continue
      const mentioned = part.startsWith('@') ? actorIdOf(part.slice(1)) : null
      if (mentioned) result.push({ type: 'mention', actorId: mentioned })
      else if (part === '{task}') result.push({ type: 'work_item_ref', workItemId: featuredId })
      else result.push({ type: 'text', text: part })
    }
    return { version: 1, nodes: result }
  }
  const agentText = (text: string) =>
    text.replace(/@([a-z]+)/g, (_, login: string) => `@${login === 'me' ? meHandle : handleOf(login)}`).replace('{task}', featuredTitleRef)
  const members = (await me.get(`/api/v1/projects/${projectId}/members`)) as { members?: Array<{ actorId?: string; handle?: string }> }
  const meHandle = members.members?.find((member) => member.actorId === me.actorId)?.handle ?? 'smoke'
  const featuredTitleRef = `"${featuredTask.title.split(':')[0]}"`

  /** Posts the lines in order; returns each message's id. */
  const post = async (conversationId: string, lines: Line[], url: (who: Session) => string) => {
    const ids: string[] = []
    for (const [who, text] of lines) {
      const client = author(who)
      if (client instanceof AgentClient) {
        const sent = (await client.call('message_send', { conversationId, text: agentText(text), clientRequestId: randomUUID() })) as {
          value?: { messageId?: string; message?: { id?: string } }
        }
        ids.push(String(sent.value?.messageId ?? sent.value?.message?.id ?? ''))
      } else {
        const sent = (await client.write('POST', url(client), { content: nodes(text) })) as { message?: { id?: string } }
        ids.push(String(sent.message?.id ?? ''))
      }
    }
    return ids
  }

  // Channels.
  const channelIds = new Map<string, string>()
  for (const name of ['general', ...channels]) {
    const created = (await me.write('POST', `/api/v1/projects/${projectId}/channels`, { name })) as { conversation?: { id?: string } }
    channelIds.set(name, String(created.conversation?.id))
  }
  const channelUrl = (id: string) => () => `/api/v1/channels/${id}/messages`
  const releases = channelIds.get('releases')!
  // A few mentions of the signed-in person and a task link, for the inbox.
  const releasesLines: Line[] = releasesChannel.map(([who, text], index) =>
    index === 3 ? [who, `${text} @me the alert threshold is the riskiest of the three.`] :
    index === 5 ? [who, `${text} See {task}.`] :
    index === 12 ? [who, `${text} @me can you confirm the priority?`] : [who, text],
  )
  const releaseIds = await post(releases, releasesLines, channelUrl(releases))
  await post(channelIds.get('design-crit')!, designCritChannel, channelUrl(channelIds.get('design-crit')!))
  await post(channelIds.get('incidents')!, incidentsChannel, channelUrl(channelIds.get('incidents')!))
  await post(channelIds.get('general')!, [
    [0, 'Welcome to the Atlas platform project, Kenji and Léa! 👋'],
    [5, 'Thanks! Glad to be here.'],
    [6, 'Hi all. Where do I find the on-call runbook?'],
    [2, 'Pages → "On-call runbook: telemetry ingest". @lea ping me if anything is unclear.'],
  ], channelUrl(channelIds.get('general')!))

  // Reactions from several people on the busiest messages.
  const reactions: Array<[number, string, number[]]> = [
    [2, 'tada', [0, 1, 3, 4, 5]],
    [5, 'thumbsup', [1, 2, 4]],
    [14, 'eyes', [0, 2]],
    [17, 'check', [0, 2, 3, 6]],
    [23, 'rocket', [0, 1, 4, 5, 7]],
    [24, 'heart', [1, 2, 3]],
  ]
  for (const [index, name, who] of reactions) {
    for (const person of who) {
      await team[person].write('PUT', `/api/v1/conversations/${releases}/messages/${releaseIds[index]}/reactions/${name}`)
    }
  }
  await me.write('PUT', `/api/v1/conversations/${releases}/messages/${releaseIds[2]}/reactions/tada`)

  // A thread off the exports timeout message.
  const rootAuthor = author(releasesChannel[releasesThread.root][0]) as Session
  const thread = (await rootAuthor.write('POST', `/api/v1/channels/${releases}/threads`, {
    name: 'Exports timeout: missing index on the October partition',
    originMessageId: releaseIds[releasesThread.root],
  })) as { conversation?: { id?: string } }
  const threadId = String(thread.conversation?.id)
  await post(threadId, releasesThread.replies, () => `/api/v1/threads/${threadId}/messages`)
  await me.write('PATCH', `/api/v1/threads/${threadId}`, { status: 'Done', outcome: 'Index added; CI now checks every partition has its parent\'s indexes.' }).catch(() => undefined)

  // A DM with Amara (every message reaches the inbox), and a group DM.
  const dm = (await team[0].write('POST', '/api/v1/direct/dms', { actorId: me.actorId })) as { conversation?: { id?: string } }
  const dmId = String(dm.conversation?.id)
  await me.write('PUT', `/api/v1/conversations/${dmId}/notification-level`, { level: 'all' })
  await post(dmId, dmLines, () => `/api/v1/conversations/${dmId}/messages`)
  const group = (await team[2].write('POST', '/api/v1/direct/groups', {
    name: 'Ingest perf crew',
    memberIds: [me.actorId, team[5].actorId, team[7].actorId],
  })) as { conversation?: { id?: string } }
  const groupId = String(group.conversation?.id)
  await post(groupId, [
    [2, 'p95 is 160 ms on staging now. Want to try the bigger batch size next week?'],
    [7, 'Yes, but after the release. I don\'t want two changes in the soak.'],
  ], () => `/api/v1/conversations/${groupId}/messages`)
  for (const [index, member] of team.entries()) {
    if (index === 0) continue
    const other = (await member.write('POST', '/api/v1/direct/dms', { actorId: me.actorId })) as { conversation?: { id?: string } }
    if (index % 2 === 0) await member.write('POST', `/api/v1/conversations/${other.conversation?.id}/messages`, { content: nodes(index === 4 ? "I'm out Thursday and Friday, back Monday." : 'Got a minute later today?') })
  }

  // Pages and a decision.
  const planPage = await team[0].write('POST', `/api/v1/projects/${projectId}/pages`, { title: plan.title, body: plan.body })
  for (const [index, page] of otherPages.entries()) {
    const writer = index === 0 ? null : team[[4, 5, 7, 2][index]]
    if (writer) await writer.write('POST', `/api/v1/projects/${projectId}/pages`, page)
    else await bots.get('Muse')!.call('page_write', { projectId, title: page.title, body: page.body })
  }
  await team[0].write('POST', `/api/v1/projects/${projectId}/decisions`, {
    title: 'Ship 2026.11 without battery alerts',
    detail: 'The panel answers what support keeps asking; alerts follow in 2026.12.',
    sourceMessageId: releaseIds[5],
    sourceConversationId: releases,
  }).catch(() => undefined)

  // A whiteboard: the ingest pipeline.
  const board = (await team[7].write('POST', '/api/v1/whiteboards', {
    title: 'Telemetry ingest: how a battery sample reaches the dashboard',
    projectId,
  })) as { whiteboard?: { id?: string } }
  const whiteboardId = String(board.whiteboard?.id)
  const boxes = [
    ['robot', 'rectangle', 0, 0, 'Robot\nfirmware 4.12'],
    ['dock', 'rectangle', 0, 200, 'Charging dock'],
    ['gateway', 'diamond', 280, 90, 'Gateway'],
    ['stream', 'rectangle', 540, 90, 'Partitioned stream\n(one per region)'],
    ['ingest', 'rectangle', 820, 0, 'Ingest workers'],
    ['store', 'ellipse', 820, 200, 'telemetry_daily'],
    ['dash', 'rectangle', 1100, 90, 'Battery health\ndashboard'],
    ['note', 'text', 540, 300, 'p95 160 ms on staging (was 410)'],
  ] as const
  await team[7].write('POST', `/api/v1/whiteboards/${whiteboardId}/operations`, {
    operations: [
      ...boxes.map(([id, type, x, y, text]) => ({ op: 'add', id, type, x, y, width: type === 'text' ? undefined : 200, height: type === 'text' ? undefined : 90, text })),
      ...[['robot', 'gateway'], ['dock', 'gateway'], ['gateway', 'stream'], ['stream', 'ingest'], ['ingest', 'store'], ['store', 'dash']].map(
        ([from, to], index) => ({ op: 'connect', id: `link-${index}`, from, to }),
      ),
    ],
  })

  const runId = await seedRuns(context, me, projectId, channelIds.get('agents-at-work')!).catch((error: unknown) => {
    console.warn(`tour: no runs seeded (${error instanceof Error ? error.message : String(error)})`)
    return null
  })
  try {
    spreadTimestamps(context.databaseUrl, projectId)
  } catch (error) {
    console.warn(`tour: timestamps left as they are (${error instanceof Error ? error.message : String(error)})`)
  }

  const state: TourState = {
    projectId,
    channelId: releases,
    channelLastMessage: releasesChannel.at(-1)![1],
    dmId,
    dmLastMessage: dmLines.at(-1)![1],
    threadId,
    threadLastMessage: releasesThread.replies.at(-1)![1],
    taskId: featuredId,
    taskTitle: featuredTask.title,
    runId,
    pageId: String(planPage.id),
    pageTitle: plan.title,
    whiteboardId,
  }
  mkdirSync(path.dirname(tourStatePath), { recursive: true })
  writeFileSync(tourStatePath, JSON.stringify(state, null, 2))
  for (const member of team) await member.api.dispose()
  for (const bot of bots.values()) await bot.api.dispose()
  return state
}

/** Runs psql with values passed as variables (:'name'), never spliced into the SQL. */
function sql(databaseUrl: string, statement: string, variables: Record<string, string> = {}): string {
  const args = [databaseUrl, '-X', '-q', '-t', '-A', '-v', 'ON_ERROR_STOP=1']
  for (const [name, value] of Object.entries(variables)) args.push('-v', `${name}=${value}`)
  const result = spawnSync('psql', args, { input: statement, encoding: 'utf8' })
  if (result.status !== 0) throw new Error(result.stderr || result.error?.message || 'psql failed')
  return result.stdout.trim()
}

const runPlans = [
  { state: 'working', agent: 'Release Steward', name: 'Soak test: 2026.11 on the staging fleet', prompt: 'Run the 48-hour soak for 2026.11 on the staging fleet and report errors, memory and ingest lag every 6 hours.', events: [['run_state_changed', 'Started'], ['command_completed', 'Deployed 2026.11-rc.2 to 64 staging robots'], ['test_result', 'Hour 2: 0 new errors, ingest lag 4 s'], ['command_started', 'Collecting hour 4 metrics']] },
  { state: 'blocked', agent: 'Muse', name: 'Release notes for 2026.11', prompt: 'Draft the 2026.11 release notes from the merged pull requests and the milestone.', events: [['run_state_changed', 'Started'], ['file_change', 'Edited docs/releases/2026.11.md'], ['approval_requested', 'Wants to publish the page "Release notes: 2026.11"']] },
  { state: 'working', agent: 'Scout', name: 'Triage: customer exports timeouts', prompt: 'Find every task about the exports timeout, link duplicates and say which one to keep.', events: [['run_state_changed', 'Started'], ['tool_used', 'Searched tasks for "exports timeout"'], ['agent_note', 'Found 3 likely duplicates']] },
  { state: 'completed', agent: 'Release Steward', name: 'Add a CI check for partition indexes', prompt: 'Add a test that fails if any telemetry partition is missing an index its parent has, and add the missing index.', events: [['run_state_changed', 'Started'], ['file_change', 'Added migrations/20261104_partition_index.sql'], ['test_result', '412 tests passed'], ['run_completed', 'Opened a pull request: Add the missing partition index and a CI check']] },
  { state: 'failed', agent: 'Telemetry Analyst', name: 'Weekly fleet report', prompt: 'Make the weekly fleet health report for the Nov 3 review.', events: [['run_state_changed', 'Started'], ['command_completed', 'Queried 1,388 robots'], ['run_failed', 'The dashboard API timed out after 3 tries']] },
] as const

/**
 * Agent runs need a repository prepared from GitHub, which the API can't do
 * here (and the profile endpoint wants a signed-in harness), so they go
 * straight into the database: a harness connection, account, profile and
 * worker, a repository, then a thread per run with the run and a few steps. Returns the working run the tour opens.
 */
async function seedRuns(context: SeedContext, me: Session, projectId: string, channelId: string): Promise<string | null> {
  const db = context.databaseUrl
  const organizationId = context.organizationId
  const [connectionId, accountId, profileId, workerActorId, workerId, repositoryId] = Array.from({ length: 6 }, () => randomUUID())
  sql(db, `
    BEGIN;
    INSERT INTO provider_connections (id, project_id, kind, host, capabilities)
    VALUES (:'connection', :'project', 'demo', 'local_project_host', '{}'::jsonb);
    INSERT INTO provider_accounts (id, organization_id, owner_actor_id, adapter_kind, name, home_path, status, account_state)
    VALUES (:'account', :'organization', :'me', 'demo', 'Tour account', :'home', 'available', 'authenticated');
    INSERT INTO execution_profiles (id, project_id, provider_connection_id, owner_actor_id, name, provider_account_id, status, account_state, preferred)
    VALUES (:'profile', :'project', :'connection', :'me', 'Tour profile', :'account', 'available', 'authenticated', true);
    INSERT INTO actors (id, kind) VALUES (:'workerActor', 'execution_worker');
    INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle)
    VALUES (gen_random_uuid(), :'project', :'workerActor', 'worker', 'Fleet runner', 'fleet-runner');
    INSERT INTO execution_workers (id, project_id, actor_id, provider_connection_id, execution_profile_id, enabled)
    VALUES (:'worker', :'project', :'workerActor', :'connection', :'profile', true);
    INSERT INTO repositories (id, project_id, github_repository_id, owner, name, clone_url, default_branch, mirror_path, trusted, preparation_state, prepared_at)
    VALUES (:'repo', :'project', 4242, 'atlas-robotics', 'fleet-platform', 'https://github.com/atlas-robotics/fleet-platform.git', 'main', :'mirror', true, 'ready', now());
    COMMIT;`,
  {
    connection: connectionId, project: projectId, account: accountId, organization: organizationId, me: me.actorId,
    home: `/tmp/aw-tour-home-${accountId}`, profile: profileId, workerActor: workerActorId, worker: workerId,
    repo: repositoryId, mirror: `/tmp/aw-tour-mirror-${repositoryId}.git`,
  })

  let first: string | null = null
  for (const [index, plan] of runPlans.entries()) {
    // A personal profile only runs for its owner, so the signed-in person asks for every run.
    const requester = me
    const thread = (await requester.write('POST', `/api/v1/channels/${channelId}/threads`, { name: plan.name })) as { conversation?: { id?: string } }
    const threadId = String(thread.conversation?.id)
    await requester.write('POST', `/api/v1/threads/${threadId}/messages`, { content: { version: 1, nodes: [{ type: 'text', text: plan.prompt }] } })
    const runId = randomUUID()
    const finished = ['completed', 'failed'].includes(plan.state)
    sql(db, `
      UPDATE conversations SET repository_id = :'repo' WHERE id = :'thread';
      INSERT INTO agent_runs (id, project_id, conversation_id, execution_worker_id, requesting_actor_id, execution_profile_id, repository_id, state, prompt, authorization_snapshot, failure_reason, started_at, finished_at, created_at)
      VALUES (:'run', :'project', :'thread', :'worker', :'requester', :'profile', :'repo', :'state', :'prompt', '{}'::jsonb,
              CASE WHEN :'state' = 'failed' THEN 'The dashboard API timed out after 3 tries.' END,
              now() - make_interval(mins => :'age'::int), CASE WHEN :'finished' = 'yes' THEN now() - make_interval(mins => :'age'::int / 2) END,
              now() - make_interval(mins => :'age'::int));`,
    { repo: repositoryId, thread: threadId, run: runId, project: projectId, worker: workerId, requester: requester.actorId, profile: profileId, state: plan.state, prompt: plan.prompt, age: String(20 + index * 35), finished: finished ? 'yes' : 'no' })
    for (const [step, [kind, summary]] of plan.events.entries()) {
      sql(db, `
        WITH event AS (
          INSERT INTO agent_run_events (id, agent_run_id, conversation_id, event_key, kind, summary, payload, created_at)
          VALUES (gen_random_uuid(), :'run', :'thread', :'key', :'kind', :'summary', '{}'::jsonb, now() - make_interval(mins => :'age'::int))
          RETURNING id
        ), sequence AS (
          UPDATE conversations SET next_entry_sequence = next_entry_sequence + 1 WHERE id = :'thread' RETURNING next_entry_sequence - 1 AS value
        )
        INSERT INTO conversation_entries (id, conversation_id, sequence, kind, agent_run_event_id)
        SELECT gen_random_uuid(), :'thread', sequence.value, 'worker_event', event.id FROM event, sequence;`,
      { run: runId, thread: threadId, key: `tour:${step}`, kind, summary, age: String(20 + index * 35 - step * 4) })
    }
    if (plan.state === 'working' && !first) first = runId
  }
  return first
}

/**
 * Everything was just made, so every message says the same minute. Spread
 * each conversation's messages over the last hours, oldest first, so the
 * timeline groups and dates read like a real day; tasks get older too.
 */
function spreadTimestamps(databaseUrl: string, projectId: string) {
  // Sent messages are guarded against changes; this throwaway database lifts
  // the guard for one transaction to backdate them.
  sql(databaseUrl, `
    BEGIN;
    ALTER TABLE messages DISABLE TRIGGER messages_guard_mutation;
    WITH ordered AS (
      SELECT m.id, row_number() OVER (PARTITION BY m.conversation_id ORDER BY e.sequence DESC) AS back
      FROM messages m JOIN conversation_entries e ON e.message_id = m.id
      WHERE m.project_id = :'project' OR m.conversation_id IN (SELECT id FROM conversations WHERE kind IN ('dm', 'group'))
    )
    UPDATE messages m SET created_at = now() - make_interval(mins => (ordered.back * 7)::int)
    FROM ordered WHERE ordered.id = m.id;
    UPDATE conversation_entries e SET created_at = m.created_at FROM messages m WHERE e.message_id = m.id;
    WITH ordered AS (SELECT id, row_number() OVER (ORDER BY number DESC) AS back FROM work_items WHERE project_id = :'project')
    UPDATE work_items w SET created_at = now() - make_interval(hours => (ordered.back * 3)::int), updated_at = now() - make_interval(hours => (ordered.back)::int)
    FROM ordered WHERE ordered.id = w.id;
    ALTER TABLE messages ENABLE TRIGGER messages_guard_mutation;
    COMMIT;`,
  { project: projectId })
}
