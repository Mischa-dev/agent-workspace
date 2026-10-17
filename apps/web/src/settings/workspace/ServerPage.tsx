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
  formatDateTime,
  formatRelative,
} from '../../ui'
import { workspaceSettingsHref } from '../paths'
import type { SettingsPageProps } from '../types'
import { type BackupState, type CheckLevel, type ServerStatus, getServerStatus, mailKeys } from './mailApi'

const CHECK_TONE: Record<CheckLevel, ChipTone> = { ok: 'success', warn: 'warning', fail: 'danger' }
const CHECK_LABEL: Record<CheckLevel, string> = { ok: 'OK', warn: 'Check', fail: 'Problem' }

const BACKUP: Record<BackupState, { tone: ChipTone; label: string; text: string }> = {
  never: { tone: 'warning', label: 'Not set up', text: 'No nightly backup has run yet. docs/operations/backups.md says how to turn it on.' },
  ok: { tone: 'success', label: 'Worked', text: 'The last nightly backup worked.' },
  failed: { tone: 'danger', label: 'Failed', text: 'The last nightly backup failed. Its log is in journalctl -u agent-workspace-daily-backup.' },
  overdue: { tone: 'warning', label: 'Overdue', text: 'The last backup worked, but it ran more than a day and a half ago. Check the backup timer.' },
  unreadable: { tone: 'warning', label: 'Unknown', text: 'The backup’s status file is there but couldn’t be read.' },
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
        icon="activity"
        title="Server"
      >
        What’s running, whether it’s healthy and when it was last backed up.
      </PageHeader>
      {status.isPending ? (
        <SettingsSkeleton label="Checking the server" sections={3} />
      ) : status.isError ? (
        <ErrorState action={<Button onClick={() => void status.refetch()}>Try again</Button>} title="The server checks didn’t load">
          Check your connection and try again.
        </ErrorState>
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
            { label: 'Commit', value: <code>{status.commit === 'dev' ? 'development build' : status.commit.slice(0, 12)}</code> },
            { label: 'Environment', value: status.environment },
            { label: 'Address', value: status.publicOrigin },
            { label: 'Checked', value: formatRelative(status.checkedAt) },
          ]}
        />
      </Section>
      <Section
        description={problems ? `${problems} thing${problems === 1 ? '' : 's'} to look at. These are the checks the deploy runs before switching to a new build.` : 'The checks the deploy runs before switching to a new build.'}
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
        {last && (
          <DetailList
            items={[
              { label: 'Finished', value: `${formatDateTime(last.finishedAt)} (${formatRelative(last.finishedAt)})` },
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
                {status.mail.lastTestAt ? `Last test ${formatRelative(status.mail.lastTestAt)}.` : 'Not tested yet.'}
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
            ...(status.jobs.waiting > 0 ? [{ label: 'Oldest waiting', value: `${Math.round(status.jobs.oldestWaitingSeconds)} s` }] : []),
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
