import { useEffect, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { describeError, toastError } from '../../app/mutations'
import {
  Banner,
  Button,
  ErrorState,
  PageHeader,
  Row,
  RowList,
  Section,
  SelectField,
  SettingsSkeleton,
  StatusChip,
  Switch,
  TextField,
  formatRelative,
  showToast,
} from '../../ui'
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

const CHANNELS: { channel: NotificationChannel; field: 'inApp' | 'push' | 'email'; label: string }[] = [
  { channel: 'in_app', field: 'inApp', label: 'Inbox' },
  { channel: 'push', field: 'push', label: 'Push' },
  { channel: 'email', field: 'email', label: 'Email' },
]

/** Account › Notifications: which kinds reach you where (N4), push devices (N3), quiet hours and muted projects. */
export default function NotificationsPage({ projects }: SettingsPageProps) {
  const settings = useQuery({ queryKey: notificationKeys.settings, queryFn: getNotificationSettings })
  return (
    <>
      <PageHeader icon="inbox" title="Notifications">
        Choose what reaches you in the inbox, on your phone and desktop, and when to stay quiet.
      </PageHeader>
      {settings.isPending ? (
        <SettingsSkeleton label="Loading notification settings" sections={3} />
      ) : settings.isError ? (
        <ErrorState action={<Button onClick={() => void settings.refetch()}>Try again</Button>} title="Notification settings didn’t load">
          Check your connection and try again.
        </ErrorState>
      ) : (
        <>
          <ChannelsSection settings={settings.data} />
          <DevicesSection />
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

function ChannelsSection({ settings }: { settings: NotificationSettings }) {
  const update = useSettingsUpdate()
  const change = useMutation({
    mutationFn: ({ group, channel, enabled }: { group: NotificationGroup; channel: NotificationChannel; enabled: boolean }) =>
      setChannel(settings.organizationId, group, channel, enabled),
    onSuccess: update,
    onError: (reason) => toastError('That wasn’t saved', reason),
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
                <th scope="row">{group.label}</th>
                {CHANNELS.map((column) => {
                  const unavailable = column.channel === 'email' && !settings.emailAvailable
                  return (
                    <td key={column.channel}>
                      <input
                        aria-label={`${group.label} by ${column.label.toLowerCase()}`}
                        checked={group[column.field]}
                        disabled={unavailable || change.isPending}
                        onChange={(event) => change.mutate({ group: group.group, channel: column.channel, enabled: event.target.checked })}
                        type="checkbox"
                      />
                    </td>
                  )
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {!settings.emailAvailable ? <p className="aw-usage__at">Email isn’t set up on this server yet; your choices are kept for when it is.</p> : null}
    </Section>
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
    onSuccess: refresh,
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
      {devices.isError ? (
        <Banner tone="danger">Your devices didn’t load.</Banner>
      ) : list.length === 0 && !devices.isPending ? (
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
              meta={[
                device.serviceHost,
                device.lastSentAt ? `last push ${formatRelative(device.lastSentAt)}` : `added ${formatRelative(device.createdAt)}`,
                device.failureCount > 0 ? `${device.failureCount} failed` : null,
              ]
                .filter(Boolean)
                .join(' · ')}
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
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (startMinute == null || endMinute == null) return
    save.mutate()
  }
  return (
    <Section description="Pushes wait until quiet hours end. The inbox still collects everything." id="quiet-heading" title="Quiet hours">
      <form className="aw-card aw-formcard aw-form aw-usage__form" onSubmit={submit}>
        {save.isError ? <Banner tone="danger">{describeError(save.error).message}</Banner> : null}
        <Switch checked={enabled} label="Hold pushes during quiet hours" onChange={setEnabled} />
        <div className="aw-form__grid">
          <TextField disabled={!enabled} error={startMinute == null ? 'Use a time such as 22:00.' : null} label="From" onChange={(event) => setStart(event.target.value)} type="time" value={start} />
          <TextField disabled={!enabled} error={endMinute == null ? 'Use a time such as 07:00.' : null} label="Until" onChange={(event) => setEnd(event.target.value)} type="time" value={end} />
          <TextField hint="Such as Europe/Berlin." label="Time zone" maxLength={64} onChange={(event) => setTimeZone(event.target.value)} value={timeZone} />
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

function MutesSection({ projects, settings }: { projects: SettingsPageProps['projects']; settings: NotificationSettings }) {
  const update = useSettingsUpdate()
  const choices = projects.filter((project) => project.organizationId === settings.organizationId)
  const [projectId, setProjectId] = useState(choices[0]?.id ?? '')
  const [group, setGroup] = useState<NotificationGroup | 'all'>('runs')
  const mute = useMutation({
    mutationFn: ({ project, kind, muted }: { project: string; kind: NotificationGroup | 'all'; muted: boolean }) => setMute(project, kind, muted),
    onSuccess: update,
    onError: (reason) => toastError('That wasn’t saved', reason),
  })
  const groupLabel = (value: string) =>
    value === 'all' ? 'Everything' : (settings.groups.find((entry) => entry.group === value)?.label ?? value)
  return (
    <Section description="Mute one kind in a busy project, such as its runs, and keep its mentions." id="mutes-heading" title="Muted projects">
      {settings.mutes.length > 0 ? (
        <RowList label="Muted projects">
          {settings.mutes.map((entry) => (
            <Row
              actions={
                <Button onClick={() => mute.mutate({ project: entry.projectId, kind: entry.group, muted: false })} size="sm" variant="ghost">
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
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: notificationKeys.settings }),
    onError: (reason) => toastError('That wasn’t saved', reason),
  })
  return (
    <Section description="A summary of what you missed, by email." id="digest-heading" title="Digest">
      <SelectField
        disabled={save.isPending}
        hint={settings.emailAvailable ? undefined : 'Email isn’t set up on this server yet; this takes effect once it is.'}
        label="Send a digest"
        onChange={(event) => save.mutate(event.target.value)}
        value={settings.digest}
      >
        <option value="off">Never</option>
        <option value="daily">Daily</option>
        <option value="weekly">Weekly</option>
      </SelectField>
    </Section>
  )
}
