import { useEffect, useRef, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { describeError, toastError } from '../../app/mutations'
import { pauseChoices, pausedUntilLabel, useNotificationPause } from '../../shell/notificationPause'
import {
  Banner,
  Button,
  EmptyState,
  ErrorState,
  Icon,
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
  countLabel,
  describeLoadError,
  formatFullDate,
  formatRelative,
  showToast,
} from '../../ui'
import { accountSettingsHref, workspaceSettingsHref } from '../paths'
import { TimeZoneField } from '../TimeZoneField'
import type { SettingsPageProps } from '../types'
import {
  PushPermissionDenied,
  currentEndpointHash,
  enablePushHere,
  pushSupport,
  unsubscribeHere,
} from './pushDevice'
import {
  getNotificationSettings,
  listDevices,
  notificationKeys,
  removeDevice,
  setChannel,
  setMute,
  setPersonalSetting,
  testDevice,
  type GroupSetting,
  type NotificationChannel,
  type NotificationGroup,
  type NotificationSettings,
  type PushDevice,
} from './usageApi'
import { browserTimeZone, minuteToTime, timeToMinute } from './usageModel'
import './usage.css'
import '../../styles/pages.css'

const CHANNELS: { channel: NotificationChannel; field: 'inApp' | 'push' | 'email'; label: string }[] = [
  { channel: 'in_app', field: 'inApp', label: 'Inbox' },
  { channel: 'push', field: 'push', label: 'Push' },
  { channel: 'email', field: 'email', label: 'Email' },
]

/** Account › Notifications: which kinds reach you where (N4), push devices (N3), quiet hours and muted projects. */
export default function NotificationsPage({ projects, session, workspace }: SettingsPageProps) {
  const settings = useQuery({
    queryKey: notificationKeys.settings,
    queryFn: getNotificationSettings,
    enabled: Boolean(workspace),
  })
  // Notification settings belong to one workspace: the default one.
  const shownWorkspace =
    session.organizations.find((organization) => organization.id === settings.data?.organizationId) ?? workspace
  const ownsWorkspace = shownWorkspace?.role === 'owner'
  return (
    <>
      <PageHeader icon="inbox" title="Notifications">
        Choose what reaches you in the inbox, on your phone and desktop, and when to stay quiet
        {session.organizations.length > 1 && shownWorkspace ? ` in ${shownWorkspace.name}` : ''}.
      </PageHeader>
      {!workspace ? (
        <EmptyState icon="inbox" title="No workspace yet">
          Join or create a workspace to set notifications.
        </EmptyState>
      ) : settings.isPending ? (
        <SettingsSkeleton label="Loading notification settings" sections={3} />
      ) : settings.isError ? (
        <ErrorState error={settings.error} onRetry={() => void settings.refetch()} title="Notification settings didn’t load" />
      ) : (
        <>
          <ChannelsSection canSetUpEmail={ownsWorkspace} settings={settings.data} />
          <DevicesSection />
          <PauseSection />
          <QuietHoursSection settings={settings.data} />
          <MutesSection projects={projects} settings={settings.data} />
          <DigestSection settings={settings.data} />
        </>
      )}
    </>
  )
}

function useSettingsUpdate() {
  const queryClient = useQueryClient()
  return (updated: NotificationSettings) => queryClient.setQueryData(notificationKeys.settings, updated)
}

type ChannelChange = { group: NotificationGroup; channel: NotificationChannel; enabled: boolean }

function ChannelsSection({ canSetUpEmail, settings }: { canSetUpEmail: boolean; settings: NotificationSettings }) {
  const update = useSettingsUpdate()
  // Each box saves on its own; only the one saving waits, and its row says when it's saved.
  const [saving, setSaving] = useState<string | null>(null)
  const [savedRow, setSavedRow] = useState<NotificationGroup | null>(null)
  const savedTimer = useRef<number | undefined>(undefined)
  useEffect(() => () => window.clearTimeout(savedTimer.current), [])
  const change = useMutation({
    mutationFn: ({ group, channel, enabled }: ChannelChange) => setChannel(settings.organizationId, group, channel, enabled),
    onMutate: ({ group, channel }) => setSaving(`${group}:${channel}`),
    onSuccess: (updated, { group }) => {
      update(updated)
      setSavedRow(group)
      window.clearTimeout(savedTimer.current)
      savedTimer.current = window.setTimeout(() => setSavedRow(null), 2000)
    },
    onError: (reason, { group }) => {
      const label = settings.groups.find((entry) => entry.group === group)?.label ?? 'notification'
      toastError(`${label} choice wasn’t saved`, reason)
    },
    onSettled: () => setSaving(null),
  })
  return (
    <Section description="Turning the inbox off for a kind still lets it reach you by push." id="channels-heading" title="What reaches you where">
      <div className="aw-card aw-notify__tablewrap">
        <table className="aw-notify__table">
          <thead>
            <tr>
              <th scope="col">Kind</th>
              {CHANNELS.map((column) => (
                <th key={column.channel} scope="col">
                  {column.label}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {settings.groups.map((group: GroupSetting) => (
              <tr key={group.group}>
                <th scope="row">
                  {group.label}
                  <span aria-live="polite" className="aw-notify__saved">
                    {savedRow === group.group ? (
                      <>
                        <Icon name="check" size={12} /> Saved
                      </>
                    ) : null}
                  </span>
                </th>
                {CHANNELS.map((column) => {
                  const unavailable = column.channel === 'email' && !settings.emailAvailable
                  return (
                    <td key={column.channel}>
                      <label className="aw-notify__check">
                        <input
                          aria-label={`${group.label} by ${column.label.toLowerCase()}`}
                          checked={group[column.field]}
                          disabled={unavailable || saving === `${group.group}:${column.channel}`}
                          onChange={(event) =>
                            change.mutate({ group: group.group, channel: column.channel, enabled: event.target.checked })
                          }
                          type="checkbox"
                        />
                      </label>
                    </td>
                  )
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <EmailNote canSetUpEmail={canSetUpEmail} settings={settings} />
    </Section>
  )
}

/** Why email can't be chosen yet, or where it goes. */
function EmailNote({ canSetUpEmail, settings }: { canSetUpEmail: boolean; settings: NotificationSettings }) {
  if (settings.emailAvailable) {
    return settings.emailAddress ? <p className="aw-usage__at">Email goes to {settings.emailAddress}.</p> : null
  }
  if (!settings.mailConfigured) {
    return (
      <p className="aw-usage__at">
        This workspace doesn’t send email yet.{' '}
        {canSetUpEmail ? (
          <>
            Set it up in <Link to={workspaceSettingsHref('email')}>Workspace › Email</Link>.
          </>
        ) : (
          'A workspace owner sets it up in Workspace › Email.'
        )}{' '}
        Your choices are kept for when it does.
      </p>
    )
  }
  return (
    <p className="aw-usage__at">
      To get email, add and confirm your address in <Link to={accountSettingsHref('profile')}>Profile</Link>.
    </p>
  )
}

function DevicesSection() {
  const queryClient = useQueryClient()
  const devices = useQuery({ queryKey: notificationKeys.devices, queryFn: listDevices })
  const [hereHash, setHereHash] = useState<string | null>(null)
  const support = pushSupport()
  useEffect(() => {
    let cancelled = false
    currentEndpointHash()
      .then((hash) => {
        if (!cancelled) setHereHash(hash)
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
    }
  }, [devices.data])
  const refresh = () => void queryClient.invalidateQueries({ queryKey: notificationKeys.devices })
  const enable = useMutation({
    mutationFn: enablePushHere,
    onSuccess: () => {
      refresh()
      showToast({ tone: 'success', title: 'Push is on for this device' })
    },
  })
  const remove = useMutation({
    mutationFn: async (device: PushDevice) => {
      await removeDevice(device.id)
      if (device.endpointHash === hereHash) await unsubscribeHere().catch(() => undefined)
    },
    onSuccess: (_, device) => {
      refresh()
      showToast({ tone: 'success', title: `${device.label} removed` })
    },
    onError: (reason) => toastError('The device wasn’t removed', reason),
  })
  const test = useMutation({
    mutationFn: testDevice,
    onSuccess: ({ result }) => {
      refresh()
      const descriptions = {
        sent: 'Check the device for it.',
        gone: 'The device had stopped accepting pushes, so it was removed.',
        retry: 'The push service is busy. Try again in a moment.',
        failed: 'The push service refused it.',
      }
      showToast({ tone: result === 'sent' ? 'success' : 'danger', title: result === 'sent' ? 'Test sent' : 'Test not sent', description: descriptions[result] })
    },
    onError: (reason) => toastError('The test wasn’t sent', reason),
  })
  const list = devices.data?.devices ?? []
  const registeredHere = hereHash != null && list.some((device) => device.endpointHash === hereHash)

  return (
    <Section
      actions={
        support === 'supported' && !registeredHere ? (
          <Button busy={enable.isPending} icon="plus" onClick={() => enable.mutate()} size="sm" variant="primary">
            Turn on push here
          </Button>
        ) : null
      }
      description="Push notifications reach your phone or desktop even when the app is closed. Install the app to your home screen first on iPhone."
      id="devices-heading"
      title="Phones and desktops"
    >
      {support === 'insecure' ? <Banner tone="info">Push needs the app to be opened over HTTPS.</Banner> : null}
      {support === 'unsupported' ? <Banner tone="info">This browser can’t receive push notifications.</Banner> : null}
      {enable.isError ? (
        <Banner tone={enable.error instanceof PushPermissionDenied ? 'warning' : 'danger'}>
          {enable.error instanceof PushPermissionDenied
            ? 'Notifications are blocked for this site. Allow them in the browser’s site settings, then try again.'
            : describeError(enable.error).message}
        </Banner>
      ) : null}
      {devices.isPending ? (
        <ListSkeleton label="Loading your devices" rows={1} />
      ) : devices.isError ? (
        <Banner onRetry={() => void devices.refetch()} title="Your devices didn’t load." tone="danger">
          {describeLoadError(devices.error)}
        </Banner>
      ) : list.length === 0 ? (
        <p className="aw-usage__empty">No devices yet.</p>
      ) : (
        <RowList label="Push devices">
          {list.map((device) => (
            <Row
              actions={
                <>
                  <Button busy={test.isPending && test.variables === device.id} onClick={() => test.mutate(device.id)} size="sm">
                    Send a test
                  </Button>
                  <Button
                    busy={remove.isPending && remove.variables?.id === device.id}
                    icon="trash"
                    onClick={() => remove.mutate(device)}
                    size="sm"
                    variant="ghost"
                  >
                    Remove
                  </Button>
                </>
              }
              key={device.id}
              meta={
                <span className="aw-row__metaline aw-wrap">
                  <span>{device.serviceHost}</span>
                  {device.lastSentAt ? (
                    <span title={formatFullDate(device.lastSentAt)}>Last push {formatRelative(device.lastSentAt)}</span>
                  ) : (
                    <span title={formatFullDate(device.createdAt)}>Added {formatRelative(device.createdAt)}</span>
                  )}
                  {device.failureCount > 0 ? (
                    <span>{countLabel(device.failureCount, 'failed push', 'failed pushes')}</span>
                  ) : null}
                </span>
              }
              title={
                <>
                  {device.label} {device.endpointHash === hereHash ? <StatusChip tone="success">This device</StatusChip> : null}
                </>
              }
            />
          ))}
        </RowList>
      )}
    </Section>
  )
}

function QuietHoursSection({ settings }: { settings: NotificationSettings }) {
  const queryClient = useQueryClient()
  const quiet = settings.quietHours
  const [enabled, setEnabled] = useState(quiet.enabled)
  const [start, setStart] = useState(minuteToTime(quiet.start))
  const [end, setEnd] = useState(minuteToTime(quiet.end))
  const [timeZone, setTimeZone] = useState(quiet.timeZone ?? browserTimeZone())
  const startMinute = timeToMinute(start)
  const endMinute = timeToMinute(end)
  const save = useMutation({
    mutationFn: async () => {
      const organization = settings.organizationId
      await setPersonalSetting(organization, 'notifications.quiet_hours', enabled)
      if (startMinute != null) await setPersonalSetting(organization, 'notifications.quiet_hours_start', startMinute)
      if (endMinute != null) await setPersonalSetting(organization, 'notifications.quiet_hours_end', endMinute)
      await setPersonalSetting(organization, 'notifications.time_zone', timeZone.trim() || null)
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: notificationKeys.settings })
      showToast({ tone: 'success', title: 'Quiet hours saved' })
    },
    onError: () => void queryClient.invalidateQueries({ queryKey: notificationKeys.settings }),
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (startMinute == null || endMinute == null) return
    save.mutate()
  }
  return (
    <Section
      description="Hold pushes every night. They arrive when quiet hours end; the inbox still collects everything."
      id="quiet-heading"
      title="Quiet hours"
    >
      <form className="aw-card aw-formcard aw-form aw-usage__form" onSubmit={submit}>
        {save.isError ? (
          <Banner tone="danger">Quiet hours weren’t saved. {describeError(save.error).message}</Banner>
        ) : null}
        <Switch checked={enabled} label="Hold pushes during quiet hours" onChange={setEnabled} />
        <div className="aw-form__grid">
          <TextField disabled={!enabled} error={startMinute == null ? 'Use a time such as 22:00.' : null} label="From" onChange={(event) => setStart(event.target.value)} type="time" value={start} />
          <TextField disabled={!enabled} error={endMinute == null ? 'Use a time such as 07:00.' : null} label="Until" onChange={(event) => setEnd(event.target.value)} type="time" value={end} />
          <TimeZoneField onChange={setTimeZone} value={timeZone} />
        </div>
        <div className="aw-form__actions">
          <Button busy={save.isPending} type="submit" variant="primary">
            Save quiet hours
          </Button>
        </div>
      </form>
    </Section>
  )
}

/** Pause by hand, the same as from your avatar menu. Takes effect at once. */
function PauseSection() {
  const pause = useNotificationPause()
  if (!pause.available) return null
  return (
    <Section
      description="Pushes wait until the pause ends and emails aren’t sent; the inbox still collects everything."
      id="pause-heading"
      title="Pause notifications"
    >
      {pause.until ? (
        <Banner
          action={
            <Button onClick={() => void pause.resume().catch((reason) => toastError('Notifications didn’t resume', reason))} size="sm">
              Resume
            </Button>
          }
          tone="info"
        >
          Notifications are paused until {pausedUntilLabel(pause.until, new Date())}.
        </Banner>
      ) : (
        <div className="aw-form__actions aw-usage__pause" role="group" aria-label="Pause notifications">
          {pauseChoices(new Date()).map((choice) => (
            <Button
              icon="moon"
              key={choice.id}
              onClick={() => void pause.pause(choice.until).catch((reason) => toastError('Notifications weren’t paused', reason))}
              size="sm"
            >
              Pause {choice.label.charAt(0).toLowerCase()}
              {choice.label.slice(1)}
            </Button>
          ))}
        </div>
      )}
    </Section>
  )
}

function MutesSection({ projects, settings }: { projects: SettingsPageProps['projects']; settings: NotificationSettings }) {
  const update = useSettingsUpdate()
  const choices = projects.filter((project) => project.organizationId === settings.organizationId)
  // Projects can arrive after the first render, so the first one is the
  // choice until someone picks another.
  const [picked, setProjectId] = useState('')
  const projectId = choices.some((project) => project.id === picked) ? picked : (choices[0]?.id ?? '')
  const [group, setGroup] = useState<NotificationGroup | 'all'>('runs')
  const groupLabel = (value: string) =>
    value === 'all' ? 'Everything' : (settings.groups.find((entry) => entry.group === value)?.label ?? value)
  const projectName = (id: string) =>
    choices.find((project) => project.id === id)?.name ??
    settings.mutes.find((entry) => entry.projectId === id)?.projectName ??
    'the project'
  const mute = useMutation({
    mutationFn: ({ project, kind, muted }: { project: string; kind: NotificationGroup | 'all'; muted: boolean }) => setMute(project, kind, muted),
    onSuccess: (updated, { project, kind, muted }) => {
      update(updated)
      showToast({
        tone: 'success',
        title: `${groupLabel(kind)} in ${projectName(project)} ${muted ? 'muted' : 'unmuted'}`,
        onUndo: () => mute.mutate({ project, kind, muted: !muted }),
      })
    },
    onError: (reason, { muted }) => toastError(muted ? 'The project wasn’t muted' : 'The project wasn’t unmuted', reason),
  })
  return (
    <Section description="Mute one kind in a busy project, such as its runs, and keep its mentions." id="mutes-heading" title="Muted projects">
      {settings.mutes.length > 0 ? (
        <RowList label="Muted projects">
          {settings.mutes.map((entry) => (
            <Row
              actions={
                <Button
                  disabled={mute.isPending && mute.variables?.project === entry.projectId && mute.variables.kind === entry.group}
                  onClick={() => mute.mutate({ project: entry.projectId, kind: entry.group, muted: false })}
                  size="sm"
                  variant="ghost"
                >
                  Unmute
                </Button>
              }
              key={`${entry.projectId}:${entry.group}`}
              meta={groupLabel(entry.group)}
              title={entry.projectName}
            />
          ))}
        </RowList>
      ) : null}
      {choices.length > 0 ? (
        <form
          className="aw-usage__muteform"
          onSubmit={(event) => {
            event.preventDefault()
            if (projectId) mute.mutate({ project: projectId, kind: group, muted: true })
          }}
        >
          <SelectField label="Project" onChange={(event) => setProjectId(event.target.value)} value={projectId}>
            {choices.map((project) => (
              <option key={project.id} value={project.id}>
                {project.name}
              </option>
            ))}
          </SelectField>
          <SelectField label="Mute" onChange={(event) => setGroup(event.target.value as NotificationGroup | 'all')} value={group}>
            <option value="all">Everything</option>
            {settings.groups.map((entry) => (
              <option key={entry.group} value={entry.group}>
                {entry.label}
              </option>
            ))}
          </SelectField>
          <Button busy={mute.isPending} type="submit">
            Mute
          </Button>
        </form>
      ) : null}
    </Section>
  )
}

function DigestSection({ settings }: { settings: NotificationSettings }) {
  const queryClient = useQueryClient()
  const save = useMutation({
    mutationFn: (value: string) => setPersonalSetting(settings.organizationId, 'notifications.digest', value),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: notificationKeys.settings })
      showToast({ tone: 'success', title: 'Digest saved' })
    },
    onError: (reason) => toastError('The digest choice wasn’t saved', reason),
  })
  return (
    <Section
      description="Instead of an email for each notification you chose email for, one email a day or week with the ones you haven’t read."
      id="digest-heading"
      title="Digest"
    >
      <SelectField
        disabled={save.isPending}
        hint={settings.emailAvailable ? undefined : 'Email doesn’t reach you yet; this takes effect once it does.'}
        label="Send a digest"
        onChange={(event) => save.mutate(event.target.value)}
        value={settings.digest}
      >
        <option value="off">Never (email each one)</option>
        <option value="daily">Daily</option>
        <option value="weekly">Weekly</option>
      </SelectField>
    </Section>
  )
}
