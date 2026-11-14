import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { toastError } from '../app/mutations'
import { useHomeExperience } from '../experience/layout'
import { isExternalHref, useInbox, useUpdateInbox, type InboxNotification } from '../inbox/api'
import { getOnboarding, onboardingKeys, progressText, STEP_TITLES, updateOnboarding } from '../onboarding/api'
import { listUsage, usageKeys, type AccountUsage, type UsageWindow } from '../settings/account/usageApi'
import { formatCountdown, useNow, windowTone } from '../settings/account/usageModel'
import { Button, ButtonLink, ErrorState, ListSkeleton, Row, RowList, Section, countLabel, formatFullDate, formatRelative } from '../ui'
import { getMyWork, viewKeys, type MyWork } from '../views/api'
import type { Piece, Thing, ThingDecl } from '../modules/contract'
import { PieceView } from '../pieces/Pieces'
import { things as runThings } from '../../../../modules/runs.json'
import { things as taskThings } from '../../../../modules/tasks.json'
import { getHome, homeKeys, type WorkingRun } from './homeApi'
import type { QuietSections } from './homeQuiet'
import { MoreLink } from './MoreLink'

const MAX_ROWS = 5

/** The empty sections, folded into one line: "Nothing needs you, no agents are working and nothing is assigned to you." */
export function QuietLine({ quiet, sections }: { quiet: QuietSections; sections: Array<keyof QuietSections> }) {
  const experience = useHomeExperience()
  const shown = sections.filter((section) => quiet[section])
  if (!shown.length) return null
  const phrases: Record<Exclude<keyof QuietSections, 'capacity'>, string> = {
    needsYou: 'nothing needs you',
    working: 'no agents are working',
    tasks: `no ${experience.word('task', 'many')} are assigned to you`,
  }
  const said = shown.filter((section) => section !== 'capacity').map((section) => phrases[section as Exclude<keyof QuietSections, 'capacity'>])
  const sentence = said.length
    ? `${said.length > 1 ? `${said.slice(0, -1).join(', ')} and ${said.at(-1)}` : said[0]}.`
    : ''
  return (
    <p className="aw-home__allquiet" role="status">
      <span>{sentence ? sentence.charAt(0).toUpperCase() + sentence.slice(1) : null}</span>
      {shown.includes('capacity') ? (
        <span>
          {' '}
          No accounts for runs yet: <Link to="/settings/account/provider-accounts">add one</Link> to run agents and see their limits.
        </span>
      ) : null}
    </p>
  )
}

/** The first-run path, until it's done or put away (P5). */
export function SetupCard({ organizationId }: { organizationId: string }) {
  const queryClient = useQueryClient()
  const onboarding = useQuery({ queryKey: onboardingKeys.workspace(organizationId), queryFn: () => getOnboarding(organizationId) })
  const hide = useMutation({
    mutationFn: () => updateOnboarding(organizationId, { dismissed: true }),
    onSuccess: (updated) => queryClient.setQueryData(onboardingKeys.workspace(organizationId), updated),
    onError: (reason) => toastError('Setup wasn’t hidden', reason),
  })
  const data = onboarding.data
  if (!data || data.complete || data.dismissed || !data.nextStep) return null
  return (
    <section aria-labelledby="home-setup" className="aw-home__setup">
      <div>
        <h2 className="aw-home__setup-title" id="home-setup">
          {data.canManageWorkspace ? `Set up ${data.organizationName}` : `Get started in ${data.organizationName}`}
        </h2>
        <p className="aw-home__setup-text">
          {progressText(data)}. Next: {STEP_TITLES[data.nextStep]}.
        </p>
      </div>
      <div className="aw-home__setup-actions">
        <Button busy={hide.isPending} onClick={() => hide.mutate()} variant="ghost">
          Hide
        </Button>
        <ButtonLink to="/welcome" variant="primary">
          Continue setup
        </ButtonLink>
      </div>
    </section>
  )
}

/** What needs you: the newest unread from the inbox. Opening one marks it read, as the Inbox does. */
export function NeedsYou() {
  const inbox = useInbox('unread', 'all')
  const update = useUpdateInbox('unread', 'all')
  const items = inbox.data?.pages[0]?.notifications ?? []
  const unread = inbox.data?.pages[0]?.unreadCount ?? 0
  const markRead = (item: InboxNotification) => {
    if (!item.read) update.mutate({ action: 'read', ids: [item.id] })
  }
  return (
    <Section actions={<MoreLink to="/inbox">Inbox</MoreLink>} id="home-needs-you" title="Needs you">
      {inbox.isPending ? (
        <ListSkeleton label="Loading what needs you" rows={3} />
      ) : inbox.isError ? (
        <ErrorState error={inbox.error} onRetry={() => void inbox.refetch()} title="The inbox didn’t load" />
      ) : items.length === 0 ? (
        <p className="aw-home__quiet">Nothing in your inbox is waiting on you.</p>
      ) : (
        <>
          <RowList label="Needs you">
            {items.slice(0, MAX_ROWS).map((item) => (
              <Row
                className="aw-row--link"
                key={item.id}
                meta={
                  <span title={formatFullDate(item.at)}>
                    {item.projectName} · {formatRelative(item.at)}
                  </span>
                }
                title={
                  isExternalHref(item.href) ? (
                    <a className="aw-row__link" href={item.href} onClick={() => markRead(item)} rel="noopener noreferrer" target="_blank">
                      {item.title}
                    </a>
                  ) : (
                    <Link className="aw-row__link" onClick={() => markRead(item)} to={item.href}>
                      {item.title}
                    </Link>
                  )
                }
              />
            ))}
          </RowList>
          {unread > MAX_ROWS ? <p className="aw-home__more">And {unread - MAX_ROWS} more in the inbox.</p> : null}
        </>
      )}
    </Section>
  )
}

const RUN_STATES: Record<string, string> = {
  queued: 'Waiting for a slot',
  starting: 'Starting',
  working: 'Working',
  blocked: 'Waiting on you',
  recovering: 'Recovering',
}

const RUN_THING = runThings.find((thing) => thing.kind === 'run') as ThingDecl
const RUN_ROWS: Piece = { type: 'list', thing: 'run', fields: ['state', 'project', 'harness', 'model', 'startedAt', 'startedBy'] }

/** A run working now, as the list piece draws it: its task (or thread), where, on what, and since when. */
function runThing(run: WorkingRun, keys: boolean): Thing {
  return {
    id: run.id,
    kind: 'run',
    module: 'runs',
    key: keys ? run.task?.key : undefined,
    title: run.task ? run.task.title : run.threadName,
    projectId: run.projectId,
    href: run.href,
    fields: {
      state: [run.approvalsWaiting > 0 ? 'Needs approval' : (RUN_STATES[run.state] ?? 'Working')],
      project: run.projectName,
      harness: run.harnessName,
      model: run.model,
      startedAt: run.startedAt ?? run.createdAt,
      startedBy: run.mine ? 'you' : run.startedBy,
    },
  }
}

const waitingText = (queued: number) => `${countLabel(queued, 'run')} waiting for a slot`

/** Agents working now, across every project you can open. */
export function WorkingNow() {
  const home = useQuery({ queryKey: homeKeys.summary, queryFn: getHome, refetchInterval: 30_000 })
  const experience = useHomeExperience()
  const keys = experience.shows('tech.task_keys')
  return (
    <Section actions={experience.shows('workspace.runs') ? <MoreLink to="/runs">Runs</MoreLink> : null} id="home-working" title="Working now">
      {home.isPending ? (
        <ListSkeleton label="Loading runs" rows={2} />
      ) : home.isError ? (
        <ErrorState error={home.error} onRetry={() => void home.refetch()} title="Runs didn’t load" />
      ) : home.data.working.length === 0 ? (
        <p className="aw-home__quiet">
          No agents are working right now.{' '}
          {home.data.queuedLaunches > 0 ? `${waitingText(home.data.queuedLaunches)}.` : 'Start one from any task’s page.'}
        </p>
      ) : (
        <>
          <PieceView decl={RUN_THING} label="Working now" piece={RUN_ROWS} things={home.data.working.map((run) => runThing(run, keys))} />
          {home.data.workingTotal > home.data.working.length || home.data.queuedLaunches > 0 ? (
            <p className="aw-home__more">
              {[
                home.data.workingTotal > home.data.working.length ? `${home.data.workingTotal - home.data.working.length} more working` : null,
                home.data.queuedLaunches > 0 ? waitingText(home.data.queuedLaunches) : null,
              ]
                .filter(Boolean)
                .join(' · ')}
              .
            </p>
          ) : null}
        </>
      )}
    </Section>
  )
}

/** The window that resets soonest; one whose reset has passed counts as resetting now. */
function nextWindow(account: AccountUsage, now: number): UsageWindow | null {
  return (
    [...account.windows]
      .filter((window) => window.resetsAt && Date.parse(window.resetsAt) > now)
      .sort((left, right) => Date.parse(left.resetsAt ?? '') - Date.parse(right.resetsAt ?? ''))[0] ??
    account.windows[0] ??
    null
  )
}

function resetLabel(resetsInMs: number) {
  return resetsInMs <= 0 ? 'Resetting now' : `Resets in ${formatCountdown(resetsInMs).replace(/ \d+ s$/, '')}`
}

/** Each account's soonest window, counting down to its reset. */
export function Capacity() {
  const usage = useQuery({ queryKey: usageKeys.list, queryFn: listUsage, refetchInterval: 60_000 })
  const now = useNow(30_000)
  const accounts = usage.data?.accounts ?? []
  return (
    <Section actions={<MoreLink to="/settings/account/usage">Usage</MoreLink>} id="home-capacity" title="Capacity">
      {usage.isPending ? (
        <ListSkeleton label="Loading capacity" rows={2} />
      ) : usage.isError ? (
        <ErrorState error={usage.error} onRetry={() => void usage.refetch()} title="Usage didn’t load" />
      ) : accounts.length === 0 ? (
        <p className="aw-home__quiet">
          No accounts for runs yet. <Link to="/settings/account/provider-accounts">Add one</Link> to run agents and see their limits
          here.
        </p>
      ) : (
        <ul aria-label="Capacity" className="aw-home__capacity">
          {accounts.map((account) => {
            const window = nextWindow(account, now)
            const tone = window ? windowTone(window) : 'unknown'
            const resets = window?.resetsAt ? Date.parse(window.resetsAt) - now : null
            return (
              <li className={`aw-home__window is-${tone}`} key={account.accountId}>
                <Link className="aw-home__window-link" to={`/settings/account/usage?account=${encodeURIComponent(account.accountId)}`}>
                  <span className="aw-home__window-name">
                    {account.name}
                    {window ? <span className="aw-home__window-label"> · {window.label}</span> : null}
                  </span>
                  <span className="aw-home__window-left">
                    {window?.unusedPercent != null ? `${window.unusedPercent}% left` : account.connected ? 'No usage yet' : 'Signed out'}
                  </span>
                  {window?.usedPercent != null ? (
                    <span aria-hidden="true" className="aw-home__meter">
                      <span style={{ width: `${Math.min(100, Math.max(0, window.usedPercent))}%` }} />
                    </span>
                  ) : null}
                  {resets != null ? <span className="aw-home__window-reset">{resetLabel(resets)}</span> : null}
                </Link>
              </li>
            )
          })}
        </ul>
      )}
    </Section>
  )
}

/**
 * Tasks assigned to you or claimed by you, each once. How many more there are
 * is exact only when every task came back; past that, a task both assigned to
 * and claimed by you would count twice, so it just points to My work.
 */
function moreTasks(work: MyWork, shown: number, unique: number) {
  const complete = work.assigned.items.length >= work.assigned.total && work.claimed.items.length >= work.claimed.total
  if (complete) return unique > shown ? `And ${countLabel(unique - shown, 'more task', 'more tasks')} in My work.` : null
  return 'More in My work.'
}

const TASK_THING = taskThings.find((thing) => thing.kind === 'task') as ThingDecl
const TASK_ROWS: Piece = { type: 'list', thing: 'task', fields: ['flags', 'project', 'status', 'priority'] }

/** One of your tasks, as the list piece draws it: what needs attention first, then where it is and how urgent. */
function taskThing(item: MyWork['assigned']['items'][number], keys: boolean): Thing {
  return {
    id: item.id,
    kind: 'task',
    module: 'tasks',
    key: keys ? item.key : undefined,
    title: item.title,
    status: item.statusName,
    projectId: item.projectId,
    href: `/projects/${encodeURIComponent(item.projectId)}/work-items/${encodeURIComponent(item.id)}`,
    fields: {
      flags: item.overdue ? ['Overdue'] : item.blocked ? ['Blocked'] : [],
      project: item.projectName,
      priority: item.priority && item.priority !== 'no_priority' ? item.priority : null,
    },
  }
}

export function YourTasks() {
  const work = useQuery({ queryKey: viewKeys.myWork, queryFn: getMyWork })
  const items = work.data ? [...work.data.claimed.items, ...work.data.assigned.items] : []
  const seen = new Set<string>()
  const unique = items.filter((item) => (seen.has(item.id) ? false : (seen.add(item.id), true)))
  const more = work.data ? moreTasks(work.data, Math.min(unique.length, MAX_ROWS), unique.length) : null
  const experience = useHomeExperience()
  const keys = experience.shows('tech.task_keys')
  return (
    <Section actions={<MoreLink to="/my-work">My work</MoreLink>} id="home-tasks" title={`Your ${experience.word('task', 'many')}`}>
      {work.isPending ? (
        <ListSkeleton label="Loading your tasks" rows={3} />
      ) : work.isError ? (
        <ErrorState error={work.error} onRetry={() => void work.refetch()} title="Your tasks didn’t load" />
      ) : unique.length === 0 ? (
        <p className="aw-home__quiet">Nothing is assigned to you. Tasks assigned to you or claimed by you show here.</p>
      ) : (
        <>
          <PieceView decl={TASK_THING} label="Your tasks" piece={TASK_ROWS} things={unique.slice(0, MAX_ROWS).map((item) => taskThing(item, keys))} />
          {more ? <p className="aw-home__more">{more}</p> : null}
        </>
      )}
    </Section>
  )
}
