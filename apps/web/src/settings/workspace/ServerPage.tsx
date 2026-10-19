import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import {
  Button,
  DetailList,
  ErrorState,
  PageHeader,
  Row,
  RowList,
  Section,
  SettingsSkeleton,
  StatusChip,
  type ChipTone,
  countLabel,
  formatDateTime,
  formatFullDate,
  formatRelative,
} from '../../ui'
import { workspaceSettingsHref } from '../paths'
import type { SettingsPageProps } from '../types'
import { type BackupState, type CheckLevel, type ServerStatus, getServerStatus, mailKeys } from './mailApi'

const CHECK_TONE: Record<CheckLevel, ChipTone> = { ok: 'success', warn: 'warning', fail: 'danger' }
const CHECK_LABEL: Record<CheckLevel, string> = { ok: 'OK', warn: 'Check', fail: 'Problem' }

const BACKUP: Record<BackupState, { tone: ChipTone; label: string; text: string; detail?: { summary: string; code: string; text: string } }> = {
  never: {
    tone: 'warning',
    label: 'Not set up',
    text: 'No nightly backup has run yet.',
    detail: { summary: 'How to turn it on', code: 'docs/operations/backups.md', text: 'The backup guide in the repository has the steps:' },
  },
  ok: { tone: 'success', label: 'Worked', text: 'The last nightly backup worked.' },
  failed: {
    tone: 'danger',
    label: 'Failed',
    text: 'The last nightly backup failed.',
    detail: { summary: 'Where to find its log', code: 'journalctl -u agent-workspace-daily-backup', text: 'On the server, run:' },
  },
  overdue: { tone: 'warning', label: 'Overdue', text: 'The last backup worked, but it ran more than a day and a half ago. Check the backup timer.' },
  unreadable: { tone: 'warning', label: 'Unknown', text: 'The backup’s status file is there but couldn’t be read.' },
}

/** “3 minutes”, “2 hours, 5 minutes”, “4 days, 2 hours”. */
function waitedFor(seconds: number) {
  const minutes = Math.floor(seconds / 60)
  if (minutes < 1) return countLabel(Math.max(0, Math.round(seconds)), 'second')
  if (minutes < 60) return countLabel(minutes, 'minute')
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return minutes % 60 ? `${countLabel(hours, 'hour')}, ${countLabel(minutes % 60, 'minute')}` : countLabel(hours, 'hour')
  const days = Math.floor(hours / 24)
  return hours % 24 ? `${countLabel(days, 'day')}, ${countLabel(hours % 24, 'hour')}` : countLabel(days, 'day')
}

/** Workspace › Server: the running build, the server’s own checks, the nightly backup and email. Owners only. */
export default function ServerPage({ workspace }: SettingsPageProps) {
  const organizationId = workspace?.id ?? ''
  const status = useQuery({
    queryKey: mailKeys.server(organizationId),
    queryFn: () => getServerStatus(organizationId),
    enabled: Boolean(organizationId),
    staleTime: 30_000,
  })
  if (!workspace) return null
  return (
    <>
      <PageHeader
        actions={
          <Button busy={status.isFetching} icon="retry" onClick={() => void status.refetch()}>
            Check again
          </Button>
        }
        icon="info"
        title="Server"
      >
        What’s running, whether it’s healthy and when it was last backed up.
      </PageHeader>
      {status.isPending ? (
        <SettingsSkeleton label="Checking the server" sections={3} />
      ) : status.isError ? (
        <ErrorState error={status.error} onRetry={() => void status.refetch()} title="The server checks didn’t load" />
      ) : (
        <ServerDetails status={status.data} />
      )}
    </>
  )
}

function ServerDetails({ status }: { status: ServerStatus }) {
  const backup = BACKUP[status.backup.state]
  const last = status.backup.last
  const problems = status.checks.filter((check) => check.level !== 'ok').length
  return (
    <>
      <Section id="server-build-heading" title="Running">
        <DetailList
          items={[
            { label: 'Version', value: status.version },
            { label: 'Commit', value: status.commit === 'dev' ? 'Development build' : <code>{status.commit.slice(0, 12)}</code> },
            { label: 'Environment', value: status.environment },
            { label: 'Address', value: status.publicOrigin },
            { label: 'Checked', value: <span title={formatFullDate(status.checkedAt)}>{formatRelative(status.checkedAt)}</span> },
          ]}
        />
      </Section>
      <Section
        description={
          problems
            ? `${countLabel(problems, 'thing')} to look at. These are the checks the deploy runs before switching to a new build.`
            : 'The checks the deploy runs before switching to a new build.'
        }
        id="server-checks-heading"
        title="Checks"
      >
        <RowList label="Server checks">
          {status.checks.map((check, index) => (
            <Row key={`${index}-${check.text}`} leading={<StatusChip tone={CHECK_TONE[check.level]}>{CHECK_LABEL[check.level]}</StatusChip>} title={check.text} />
          ))}
        </RowList>
      </Section>
      <Section id="server-backup-heading" title="Nightly backup">
        <p>
          <StatusChip tone={backup.tone}>{backup.label}</StatusChip> {backup.text}
        </p>
        {backup.detail ? (
          <details className="aw-disclosure">
            <summary>{backup.detail.summary}</summary>
            <p>
              {backup.detail.text} <code>{backup.detail.code}</code>
            </p>
          </details>
        ) : null}
        {last && (
          <DetailList
            items={[
              { label: 'Finished', value: <span title={formatFullDate(last.finishedAt)}>{`${formatDateTime(last.finishedAt)} (${formatRelative(last.finishedAt)})`}</span> },
              ...Object.entries(last.steps).map(([step, result]) => ({ label: stepLabel(step), value: result === 'ok' ? 'Worked' : 'Failed' })),
            ]}
          />
        )}
      </Section>
      <Section id="server-email-heading" title="Email">
        <p>
          {status.mail.configured ? (
            status.mail.enabled ? (
              <>
                <StatusChip tone={status.mail.lastTestOk === false ? 'warning' : 'success'}>{status.mail.lastTestOk === false ? 'Last test failed' : 'Set up'}</StatusChip>{' '}
                {status.mail.lastTestAt ? <span title={formatFullDate(status.mail.lastTestAt)}>Last test {formatRelative(status.mail.lastTestAt)}.</span> : 'Not tested yet.'}
              </>
            ) : (
              <><StatusChip tone="warning">Turned off</StatusChip> Sending is turned off.</>
            )
          ) : (
            <><StatusChip tone="warning">Not set up</StatusChip> Password resets and email notifications need it.</>
          )}{' '}
          <Link to={workspaceSettingsHref('email')}>Email settings</Link>
        </p>
      </Section>
      <Section id="server-jobs-heading" title="Background jobs">
        <DetailList
          items={[
            { label: 'Waiting', value: String(status.jobs.waiting) },
            { label: 'Running', value: String(status.jobs.running) },
            { label: 'Failed in the last day', value: String(status.jobs.failedLastDay) },
            ...(status.jobs.waiting > 0 ? [{ label: 'Oldest waiting', value: waitedFor(status.jobs.oldestWaitingSeconds) }] : []),
          ]}
        />
      </Section>
    </>
  )
}

function stepLabel(step: string) {
  switch (step) {
    case 'key':
      return 'Secrets key'
    case 'database':
      return 'Database and files'
    case 'keyAgain':
      return 'Secrets key, again'
    default:
      return step
  }
}
