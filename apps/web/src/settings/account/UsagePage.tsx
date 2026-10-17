import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useSearchParams } from 'react-router-dom'

import { describeError, toastError } from '../../app/mutations'
import { QuickLaunch } from '../../launch/QuickLaunch'
import {
  Banner,
  Button,
  Dialog,
  EmptyState,
  ErrorState,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  Section,
  SelectField,
  SettingsSkeleton,
  StatusChip,
  Switch,
  TextField,
  formatDateTime,
  formatRelative,
  showToast,
} from '../../ui'
import {
  addSchedule,
  listUsage,
  readyTasks,
  refreshUsage,
  removeSchedule,
  setRule,
  spendResetCredit,
  usageKeys,
  type AccountUsage,
  type AlertRule,
  type NewSchedule,
  type UsageWindow,
} from './usageApi'
import {
  browserTimeZone,
  describeSchedule,
  newAttemptKey,
  resetText,
  timeToMinute,
  useNow,
  weekdays,
  windowTone,
} from './usageModel'
import './usage.css'

const RULE_LABELS: Record<AlertRule['rule'], string> = {
  reset_soon: 'Capacity left before a reset',
  high_usage: 'Nearly used up',
  limit_reached: 'Limit reached',
  reset_credits: 'Banked resets',
}

const FAILURES: Record<string, string> = {
  unsupported: 'This harness doesn’t report usage. Add a reset schedule below to track it yourself.',
  busy: 'The harness was busy the last time we checked. We’ll try again shortly.',
  configuration: 'The account’s settings changed. Check it under Provider accounts.',
  unavailable: 'The harness couldn’t be reached the last time we checked.',
}

/** Account › Usage and alerts: every usage window with a live countdown (U1), alert rules (U3) and banked resets (U6). */
export default function UsagePage() {
  const usage = useQuery({ queryKey: usageKeys.list, queryFn: listUsage, refetchInterval: 60_000 })
  const [params] = useSearchParams()
  const focused = params.get('account')
  const showReady = params.get('ready') === '1'
  const accounts = [...(usage.data?.accounts ?? [])].sort(
    (left, right) => Number(right.accountId === focused) - Number(left.accountId === focused),
  )

  return (
    <>
      <PageHeader icon="activity" title="Usage and alerts">
        How much of each plan’s limits is left, when they reset, and when to tell you.
      </PageHeader>
      {usage.isPending ? (
        <SettingsSkeleton label="Loading usage" sections={2} />
      ) : usage.isError ? (
        <ErrorState action={<Button onClick={() => void usage.refetch()}>Try again</Button>} title="Usage didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : accounts.length === 0 ? (
        <EmptyState
          action={
            <Link className="aw-btn aw-btn--primary aw-btn--md" to="/settings/account/provider-accounts">
              <span className="aw-btn__label">Add a provider account</span>
            </Link>
          }
          icon="activity"
          title="No provider accounts yet"
        >
          Once you sign in to a harness, its usage windows show here.
        </EmptyState>
      ) : (
        accounts.map((account) => (
          <AccountUsageSection
            account={account}
            focused={account.accountId === focused}
            key={account.accountId}
            openReady={showReady && account.accountId === focused}
          />
        ))
      )}
    </>
  )
}

function useAccountUpdate() {
  const queryClient = useQueryClient()
  return (updated: AccountUsage) => {
    queryClient.setQueryData<{ accounts: AccountUsage[] }>(usageKeys.list, (current) =>
      current
        ? { accounts: current.accounts.map((account) => (account.accountId === updated.accountId ? updated : account)) }
        : current,
    )
  }
}

function AccountUsageSection({ account, focused, openReady }: { account: AccountUsage; focused: boolean; openReady: boolean }) {
  const update = useAccountUpdate()
  const refresh = useMutation({
    mutationFn: () => refreshUsage(account.accountId),
    onSuccess: update,
    onError: (reason) => toastError('Usage didn’t refresh', reason),
  })
  const section = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (focused) section.current?.scrollIntoView?.({ block: 'start' })
  }, [focused])
  const description = [account.harnessName, account.plan ? `${account.plan} plan` : null, account.polledAt ? `checked ${formatRelative(account.polledAt)}` : null]
    .filter(Boolean)
    .join(' · ')

  return (
    <div className={focused ? 'aw-usage is-focused' : 'aw-usage'} ref={section}>
      <Section
        actions={
          account.connected ? (
            <Button busy={refresh.isPending} icon="retry" onClick={() => refresh.mutate()} size="sm">
              Refresh
            </Button>
          ) : null
        }
        description={description}
        id={`usage-${account.accountId}`}
        title={account.name}
      >
        {!account.connected ? (
          <Banner tone="warning">This account is signed out, so its usage isn’t read. Sign in again under Provider accounts.</Banner>
        ) : account.pollFailure && account.pollFailedAt && (!account.polledAt || account.pollFailedAt > account.polledAt) ? (
          <Banner tone="warning">{FAILURES[account.pollFailure] ?? FAILURES.unavailable}</Banner>
        ) : null}
        <Windows windows={account.windows} />
        <BankedResets account={account} />
        <ReadyTasks accountId={account.accountId} initiallyOpen={openReady} />
        <Schedules account={account} />
        <Rules account={account} />
      </Section>
    </div>
  )
}

function Windows({ windows }: { windows: UsageWindow[] }) {
  const now = useNow()
  if (windows.length === 0) {
    return <p className="aw-usage__empty">No usage reported yet. If this harness doesn’t report limits, add a reset schedule below.</p>
  }
  return (
    <ul aria-label="Usage windows" className="aw-usage__windows">
      {windows.map((window) => {
        const tone = windowTone(window)
        return (
          <li className={`aw-usage__window is-${tone}`} key={window.key}>
            <div className="aw-usage__windowhead">
              <span className="aw-usage__label">{window.label}</span>
              {window.source === 'schedule' ? <StatusChip>Your schedule</StatusChip> : null}
              {window.limitReached ? <StatusChip tone="danger">Limit reached</StatusChip> : null}
              <span className="aw-usage__amount">
                {window.unusedPercent != null ? `${window.unusedPercent}% left` : 'Usage not reported'}
              </span>
            </div>
            {window.usedPercent != null ? (
              <div
                aria-label={`${window.label}: ${window.usedPercent}% used`}
                aria-valuemax={100}
                aria-valuemin={0}
                aria-valuenow={window.usedPercent}
                className="aw-usage__meter"
                role="meter"
              >
                <span style={{ width: `${Math.min(100, Math.max(0, window.usedPercent))}%` }} />
              </div>
            ) : null}
            <p className="aw-usage__reset">
              <span aria-live="off">{resetText(window, now)}</span>
              {window.resetsAt ? <span className="aw-usage__at"> · {formatDateTime(window.resetsAt)}</span> : null}
              {window.rolledOver ? <span className="aw-usage__at"> · reset since the last reading</span> : null}
            </p>
          </li>
        )
      })}
    </ul>
  )
}

function BankedResets({ account }: { account: AccountUsage }) {
  const update = useAccountUpdate()
  const [open, setOpen] = useState(false)
  // One key per confirmation, so a retried click never spends a second reset.
  const [attempt, setAttempt] = useState(newAttemptKey)
  const credits = account.resetCredits
  const spend = useMutation({
    mutationFn: () => spendResetCredit(account.accountId, credits?.credits[0]?.id ?? null, attempt),
    onSuccess: ({ account: updated, outcome }) => {
      update(updated)
      setOpen(false)
      setAttempt(newAttemptKey())
      const messages: Record<string, string> = {
        reset: 'Your limits were reset.',
        nothing_to_reset: 'Nothing needed resetting, so no reset was used.',
        no_credit: 'There was no banked reset to spend.',
        already_redeemed: 'That reset was already spent.',
      }
      showToast({ tone: outcome === 'reset' ? 'success' : 'info', title: 'Banked reset', description: messages[outcome] })
    },
  })
  if (!credits) return null
  const next = credits.credits[0]
  return (
    <div className="aw-usage__credits">
      <div>
        <p className="aw-usage__label">
          {credits.available === 1 ? '1 banked reset' : `${credits.available} banked resets`}
        </p>
        <p className="aw-usage__at">
          {next?.expiresAt ? `The next one expires ${formatDateTime(next.expiresAt)}.` : 'A reset refills every window at once.'}
        </p>
      </div>
      <Button disabled={credits.available === 0} icon="retry" onClick={() => setOpen(true)} size="sm">
        Spend a reset
      </Button>
      <Dialog
        description="This refills the account’s windows now. It can’t be undone."
        footer={
          <>
            <Button onClick={() => setOpen(false)} variant="ghost">
              Cancel
            </Button>
            <Button busy={spend.isPending} onClick={() => spend.mutate()} variant="primary">
              Spend it
            </Button>
          </>
        }
        onClose={() => setOpen(false)}
        open={open}
        title="Spend a banked reset?"
      >
        {spend.isError ? <Banner tone="danger">{describeError(spend.error).message}</Banner> : null}
        {next?.title ? <p>{next.title}</p> : null}
      </Dialog>
    </div>
  )
}

function ReadyTasks({ accountId, initiallyOpen }: { accountId: string; initiallyOpen: boolean }) {
  const [open, setOpen] = useState(initiallyOpen)
  const ready = useQuery({ queryKey: usageKeys.ready(accountId), queryFn: () => readyTasks(accountId), enabled: open })
  const heading = useRef<HTMLHeadingElement>(null)
  useEffect(() => {
    if (initiallyOpen) heading.current?.focus({ preventScroll: false })
  }, [initiallyOpen])
  return (
    <div className="aw-usage__block" id={initiallyOpen ? 'ready-tasks' : undefined}>
      <div className="aw-usage__blockhead">
        <h3 className="aw-usage__blocktitle" ref={heading} tabIndex={-1}>
          Ready to run
        </h3>
        <Button aria-expanded={open} onClick={() => setOpen((value) => !value)} size="sm" variant="ghost">
          {open ? 'Hide' : 'Show tasks'}
        </Button>
      </div>
      {!open ? (
        <p className="aw-usage__at">Open, unblocked tasks with a repository, in the projects this account runs for.</p>
      ) : ready.isPending ? (
        <ListSkeleton label="Loading ready tasks" rows={3} />
      ) : ready.isError ? (
        <Banner tone="danger">The ready tasks didn’t load.</Banner>
      ) : ready.data.tasks.length === 0 ? (
        <p className="aw-usage__at">Nothing is ready right now.</p>
      ) : (
        <RowList label="Ready tasks">
          {ready.data.tasks.map((task) => (
            <Row
              actions={
                <>
                  <Link className="aw-btn aw-btn--secondary aw-btn--sm" to={task.href}>
                    <span className="aw-btn__label">Open</span>
                  </Link>
                  <QuickLaunch providerAccountId={accountId} task={task} />
                </>
              }
              key={task.id}
              meta={[task.projectName, task.priority !== 'none' ? task.priority : null, task.unblocks > 0 ? `unblocks ${task.unblocks}` : null]
                .filter(Boolean)
                .join(' · ')}
              title={
                <>
                  <span className="aw-usage__key">{task.key}</span> {task.title}
                </>
              }
            />
          ))}
        </RowList>
      )}
    </div>
  )
}

function Schedules({ account }: { account: AccountUsage }) {
  const update = useAccountUpdate()
  const [adding, setAdding] = useState(false)
  const [label, setLabel] = useState('')
  const [cadence, setCadence] = useState<NewSchedule['cadence']>('weekly')
  const [weekday, setWeekday] = useState(1)
  const [monthDay, setMonthDay] = useState(1)
  const [time, setTime] = useState('09:00')
  const [timeZone, setTimeZone] = useState(browserTimeZone)
  const add = useMutation({
    mutationFn: (schedule: NewSchedule) => addSchedule(account.accountId, schedule),
    onSuccess: (updated) => {
      update(updated)
      setAdding(false)
      setLabel('')
    },
  })
  const remove = useMutation({
    mutationFn: (scheduleId: string) => removeSchedule(account.accountId, scheduleId),
    onSuccess: update,
    onError: (reason) => toastError('The schedule wasn’t removed', reason),
  })
  const minute = timeToMinute(time)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (minute == null || !label.trim()) return
    add.mutate({
      label: label.trim(),
      cadence,
      weekday: cadence === 'weekly' ? weekday : null,
      monthDay: cadence === 'monthly' ? monthDay : null,
      minuteOfDay: minute,
      timeZone: timeZone.trim() || 'UTC',
    })
  }
  return (
    <div className="aw-usage__block">
      <div className="aw-usage__blockhead">
        <h3 className="aw-usage__blocktitle">Reset schedules</h3>
        {!adding ? (
          <Button icon="plus" onClick={() => setAdding(true)} size="sm" variant="ghost">
            Add a schedule
          </Button>
        ) : null}
      </div>
      <p className="aw-usage__at">For plans the harness doesn’t report, such as a team plan that resets every Monday.</p>
      {account.schedules.length > 0 ? (
        <RowList label="Reset schedules">
          {account.schedules.map((schedule) => (
            <Row
              actions={
                <Button
                  busy={remove.isPending && remove.variables === schedule.id}
                  icon="trash"
                  onClick={() => remove.mutate(schedule.id)}
                  size="sm"
                  variant="ghost"
                >
                  Remove
                </Button>
              }
              key={schedule.id}
              meta={describeSchedule(schedule)}
              title={schedule.label}
            />
          ))}
        </RowList>
      ) : null}
      {adding ? (
        <form className="aw-card aw-formcard aw-form aw-usage__form" onSubmit={submit}>
          {add.isError ? <Banner tone="danger">{describeError(add.error).message}</Banner> : null}
          <TextField label="Name" maxLength={80} onChange={(event) => setLabel(event.target.value)} placeholder="Team plan" required value={label} />
          <div className="aw-form__grid">
            <SelectField label="Resets" onChange={(event) => setCadence(event.target.value as NewSchedule['cadence'])} value={cadence}>
              <option value="daily">Every day</option>
              <option value="weekly">Every week</option>
              <option value="monthly">Every month</option>
            </SelectField>
            {cadence === 'weekly' ? (
              <SelectField label="On" onChange={(event) => setWeekday(Number(event.target.value))} value={weekday}>
                {weekdays().map((day) => (
                  <option key={day.value} value={day.value}>
                    {day.label}
                  </option>
                ))}
              </SelectField>
            ) : cadence === 'monthly' ? (
              <TextField label="Day of the month" max={28} min={1} onChange={(event) => setMonthDay(Number(event.target.value))} type="number" value={monthDay} />
            ) : null}
            <TextField error={minute == null ? 'Use a time such as 09:00.' : null} label="At" onChange={(event) => setTime(event.target.value)} type="time" value={time} />
            <TextField label="Time zone" maxLength={64} onChange={(event) => setTimeZone(event.target.value)} value={timeZone} />
          </div>
          <div className="aw-form__actions">
            <Button onClick={() => setAdding(false)} variant="ghost">
              Cancel
            </Button>
            <Button busy={add.isPending} type="submit" variant="primary">
              Add schedule
            </Button>
          </div>
        </form>
      ) : null}
    </div>
  )
}

function Rules({ account }: { account: AccountUsage }) {
  return (
    <div className="aw-usage__block">
      <h3 className="aw-usage__blocktitle">Alerts</h3>
      <p className="aw-usage__at">
        Each alert fires once per reset. Choose where they reach you under Notifications.
      </p>
      <div className="aw-usage__rules">
        {account.rules.map((rule) => (
          <RuleRow accountId={account.accountId} key={`${rule.rule}:${rule.hours}:${rule.percent}`} rule={rule} />
        ))}
      </div>
    </div>
  )
}

function ruleSummary(rule: AlertRule) {
  switch (rule.rule) {
    case 'reset_soon':
      return `When a window resets within ${rule.hours ?? 3} h with at least ${rule.percent ?? 25}% unused.`
    case 'high_usage':
      return `When a window is ${rule.percent ?? 90}% used.`
    case 'limit_reached':
      return 'When a limit is reached, with when it resets.'
    case 'reset_credits':
      return `When a banked reset arrives or expires within ${rule.hours ?? 48} h.`
  }
}

function RuleRow({ accountId, rule }: { accountId: string; rule: AlertRule }) {
  const update = useAccountUpdate()
  const [hours, setHours] = useState(rule.hours)
  const [percent, setPercent] = useState(rule.percent)
  const save = useMutation({
    mutationFn: (change: { enabled: boolean; hours: number | null; percent: number | null }) => setRule(accountId, rule.rule, change),
    onSuccess: update,
    onError: (reason) => toastError('The alert wasn’t saved', reason),
  })
  const changed = hours !== rule.hours || percent !== rule.percent
  return (
    <div className="aw-usage__rule">
      <Switch
        checked={rule.enabled}
        description={ruleSummary(rule)}
        disabled={save.isPending}
        label={RULE_LABELS[rule.rule]}
        onChange={(enabled) => save.mutate({ enabled, hours: rule.hours, percent: rule.percent })}
      />
      {rule.enabled && (rule.defaultHours != null || rule.defaultPercent != null) ? (
        <div className="aw-usage__ruleinputs">
          {rule.defaultHours != null ? (
            <TextField
              label="Hours"
              max={168}
              min={1}
              onChange={(event) => setHours(event.target.value ? Number(event.target.value) : null)}
              type="number"
              value={hours ?? ''}
            />
          ) : null}
          {rule.defaultPercent != null ? (
            <TextField
              label="Percent"
              max={100}
              min={1}
              onChange={(event) => setPercent(event.target.value ? Number(event.target.value) : null)}
              type="number"
              value={percent ?? ''}
            />
          ) : null}
          {changed ? (
            <Button busy={save.isPending} onClick={() => save.mutate({ enabled: true, hours, percent })} size="sm" variant="primary">
              Save
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}

