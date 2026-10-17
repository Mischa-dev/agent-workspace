import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { toastError } from '../app/mutations'
import { isExternalHref, useInbox } from '../inbox/api'
import { getOnboarding, onboardingKeys, progressText, STEP_TITLES, updateOnboarding } from '../onboarding/api'
import { listUsage, usageKeys, type AccountUsage, type UsageWindow } from '../settings/account/usageApi'
import { formatCountdown, useNow, windowTone } from '../settings/account/usageModel'
import { Button, EmptyState, ErrorState, Icon, ListSkeleton, Row, RowList, Section, StatusChip, formatFullDate, formatRelative } from '../ui'
import { getMyWork, viewKeys } from '../views/api'
import { getHome, homeKeys, type WorkingRun } from './homeApi'

const MAX_ROWS = 5

function SectionError({ onRetry, title }: { onRetry: () => void; title: string }) {
  return (
    <ErrorState action={<Button onClick={onRetry}>Try again</Button>} title={title}>
      Check your connection and try again.
    </ErrorState>
  )
}

function MoreLink({ children, to }: { children: string; to: string }) {
  return (
    <Link className="aw-btn aw-btn--ghost aw-btn--sm" to={to}>
      <span className="aw-btn__label">{children}</span>
      <Icon name="chevron-right" size={14} />
    </Link>
  )
}

/** The first-run path, until it's done or put away (P5). */
export function SetupCard({ organizationId }: { organizationId: string }) {
  const queryClient = useQueryClient()
  const onboarding = useQuery({ queryKey: onboardingKeys.workspace(organizationId), queryFn: () => getOnboarding(organizationId) })
  const hide = useMutation({
    mutationFn: () => updateOnboarding(organizationId, { dismissed: true }),
    onSuccess: (updated) => queryClient.setQueryData(onboardingKeys.workspace(organizationId), updated),
    onError: (reason) => toastError('Couldn’t hide setup', reason),
  })
  const data = onboarding.data
  if (!data || data.complete || data.dismissed || !data.nextStep) return null
  return (
    <section aria-labelledby="home-setup" className="aw-home__setup">
      <div>
        <h2 className="aw-home__setup-title" id="home-setup">
          Set up {data.organizationName}
        </h2>
        <p className="aw-home__setup-text">
          {progressText(data)}. Next: {STEP_TITLES[data.nextStep].toLowerCase()}.
        </p>
      </div>
      <div className="aw-home__setup-actions">
        <Link className="aw-btn aw-btn--primary aw-btn--md" to="/welcome">
          <span className="aw-btn__label">Continue setup</span>
        </Link>
        <Button busy={hide.isPending} onClick={() => hide.mutate()} variant="ghost">
          Hide
        </Button>
      </div>
    </section>
  )
}

/** What needs you: the newest unread from the inbox. */
export function NeedsYou() {
  const inbox = useInbox('unread', 'all')
  const items = inbox.data?.pages[0]?.notifications ?? []
  const unread = inbox.data?.pages[0]?.unreadCount ?? 0
  return (
    <Section actions={<MoreLink to="/inbox">Inbox</MoreLink>} id="home-needs-you" title="Needs you">
      {inbox.isPending ? (
        <ListSkeleton label="Loading what needs you" rows={3} />
      ) : inbox.isError ? (
        <SectionError onRetry={() => void inbox.refetch()} title="The inbox didn’t load" />
      ) : items.length === 0 ? (
        <p className="aw-home__quiet">Nothing needs you. You’re all caught up.</p>
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
                  <Link className="aw-row__link" to={isExternalHref(item.href) ? '/inbox' : item.href}>
                    {item.title}
                  </Link>
                }
              />
            ))}
          </RowList>
          {unread > MAX_ROWS ? <p className="aw-home__quiet">And {unread - MAX_ROWS} more in the inbox.</p> : null}
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

function runMeta(run: WorkingRun) {
  return [run.projectName, run.harnessName, run.model, run.mine ? 'started by you' : `started by ${run.startedBy}`]
    .filter(Boolean)
    .join(' · ')
}

/** Agents working now, across every project you can open. */
export function WorkingNow() {
  const home = useQuery({ queryKey: homeKeys.summary, queryFn: getHome, refetchInterval: 30_000 })
  return (
    <Section id="home-working" title="Working now">
      {home.isPending ? (
        <ListSkeleton label="Loading runs" rows={2} />
      ) : home.isError ? (
        <SectionError onRetry={() => void home.refetch()} title="Runs didn’t load" />
      ) : home.data.working.length === 0 ? (
        <p className="aw-home__quiet">
          No agents are working right now.
          {home.data.queuedLaunches > 0 ? ` ${home.data.queuedLaunches} waiting in launch queues.` : ' Launch a task from its page.'}
        </p>
      ) : (
        <>
          <RowList label="Working now">
            {home.data.working.map((run) => (
              <Row
                actions={
                  <StatusChip tone={run.state === 'blocked' || run.approvalsWaiting > 0 ? 'warning' : 'agent'}>
                    {run.approvalsWaiting > 0 ? 'Needs approval' : (RUN_STATES[run.state] ?? run.state)}
                  </StatusChip>
                }
                className="aw-row--link"
                key={run.id}
                meta={
                  <span title={formatFullDate(run.startedAt ?? run.createdAt)}>
                    {runMeta(run)} · {formatRelative(run.startedAt ?? run.createdAt)}
                  </span>
                }
                title={
                  <Link className="aw-row__link" to={run.href}>
                    {run.task ? (
                      <>
                        <span className="aw-home__key">{run.task.key}</span> {run.task.title}
                      </>
                    ) : (
                      run.threadName
                    )}
                  </Link>
                }
              />
            ))}
          </RowList>
          {home.data.workingTotal > home.data.working.length || home.data.queuedLaunches > 0 ? (
            <p className="aw-home__quiet">
              {[
                home.data.workingTotal > home.data.working.length ? `${home.data.workingTotal - home.data.working.length} more working` : null,
                home.data.queuedLaunches > 0 ? `${home.data.queuedLaunches} waiting in launch queues` : null,
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

function nextWindow(account: AccountUsage, now: number): UsageWindow | null {
  return (
    [...account.windows]
      .filter((window) => window.resetsAt && Date.parse(window.resetsAt) > now)
      .sort((left, right) => Date.parse(left.resetsAt ?? '') - Date.parse(right.resetsAt ?? ''))[0] ??
    account.windows[0] ??
    null
  )
}

/** Each provider account's soonest window, counting down to its reset. */
export function Capacity() {
  const usage = useQuery({ queryKey: usageKeys.list, queryFn: listUsage, refetchInterval: 60_000 })
  const now = useNow(30_000)
  const accounts = usage.data?.accounts ?? []
  return (
    <Section actions={<MoreLink to="/settings/account/usage">Usage</MoreLink>} id="home-capacity" title="Capacity">
      {usage.isPending ? (
        <ListSkeleton label="Loading capacity" rows={2} />
      ) : usage.isError ? (
        <SectionError onRetry={() => void usage.refetch()} title="Usage didn’t load" />
      ) : accounts.length === 0 ? (
        <p className="aw-home__quiet">
          No provider accounts yet. <Link to="/settings/account/provider-accounts">Add one</Link> to run agents and see their limits here.
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
                    {window?.unusedPercent != null ? `${window.unusedPercent}% left` : account.connected ? 'Not reported' : 'Signed out'}
                  </span>
                  {window?.usedPercent != null ? (
                    <span aria-hidden="true" className="aw-home__meter">
                      <span style={{ width: `${Math.min(100, Math.max(0, window.usedPercent))}%` }} />
                    </span>
                  ) : null}
                  <span className="aw-home__window-reset">
                    {resets != null ? `Resets in ${formatCountdown(resets).replace(/ \d+ s$/, '')}` : 'No reset time'}
                  </span>
                </Link>
              </li>
            )
          })}
        </ul>
      )}
    </Section>
  )
}

/** Tasks assigned to you or claimed by you. */
export function YourTasks() {
  const work = useQuery({ queryKey: viewKeys.myWork, queryFn: getMyWork })
  const items = work.data ? [...work.data.claimed.items, ...work.data.assigned.items] : []
  const seen = new Set<string>()
  const unique = items.filter((item) => (seen.has(item.id) ? false : (seen.add(item.id), true)))
  const total = work.data ? work.data.assigned.total + work.data.claimed.total : 0
  return (
    <Section actions={<MoreLink to="/my-work">My work</MoreLink>} id="home-tasks" title="Your tasks">
      {work.isPending ? (
        <ListSkeleton label="Loading your tasks" rows={3} />
      ) : work.isError ? (
        <SectionError onRetry={() => void work.refetch()} title="Your tasks didn’t load" />
      ) : unique.length === 0 ? (
        <EmptyState icon="checklist" title="Nothing assigned to you">
          Tasks assigned to you or claimed by you show here.
        </EmptyState>
      ) : (
        <>
          <RowList label="Your tasks">
            {unique.slice(0, MAX_ROWS).map((item) => (
              <Row
                actions={item.overdue ? <StatusChip tone="danger">Overdue</StatusChip> : item.blocked ? <StatusChip tone="warning">Blocked</StatusChip> : null}
                className="aw-row--link"
                key={item.id}
                meta={[item.projectName, item.statusName, item.priority !== 'none' ? item.priority : null].filter(Boolean).join(' · ')}
                title={
                  <Link className="aw-row__link" to={`/projects/${item.projectId}/work-items/${item.id}`}>
                    <span className="aw-home__key">{item.key}</span> {item.title}
                  </Link>
                }
              />
            ))}
          </RowList>
          {total > MAX_ROWS ? <p className="aw-home__quiet">And {total - MAX_ROWS} more in My work.</p> : null}
        </>
      )}
    </Section>
  )
}
