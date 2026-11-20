import { useMutation, useQueryClient } from '@tanstack/react-query'

import { describeError } from '../../app/mutations'
import { useWorkspace } from '../../app/workspace'
import { useAuthenticatedSession } from '../../auth/session'
import { useOfflineGate } from '../../data/offlineGate'
import { offlineReadingOn } from '../../data/offlinePointer'
import { Banner, ErrorState, Section, SettingsSkeleton, Switch, showToast } from '../../ui'
import { settingsKeys, useEffectiveSettings, type SettingsTarget } from '../api'
import { setPersonalSetting } from './usageApi'

/**
 * Account › Security › Reading offline (W1.31): the person's own switch for
 * keeping what they recently opened on their devices, per workspace, which
 * the workspace can switch off for everyone.
 */
export function OfflineSection() {
  const workspace = useWorkspace()
  if (!workspace) return null
  return <OfflineSwitch name={workspace.name} target={{ scope: 'workspace', organizationId: workspace.id }} />
}

function OfflineSwitch({ name, target }: { name: string; target: Extract<SettingsTarget, { scope: 'workspace' }> }) {
  const queryClient = useQueryClient()
  const effective = useEffectiveSettings(target)
  // What's kept mixes every workspace the person is in, so any of them that doesn't allow it keeps nothing.
  const elsewhere = useOfflineGate(target.organizationId, useAuthenticatedSession().organizations).blockedBy.filter(
    (workspace) => workspace.id !== target.organizationId,
  )
  const write = useMutation({
    mutationFn: (on: boolean) => setPersonalSetting(target.organizationId, 'offline.reading', on),
    onSuccess: (_, on) => {
      void queryClient.invalidateQueries({ queryKey: settingsKeys.effective(target) })
      showToast({
        tone: 'success',
        title: on ? 'Reading offline is on' : 'Reading offline is off',
        description: on ? undefined : 'What this device kept is cleared.',
      })
    },
  })
  const allowed = effective.data?.settings.find((setting) => setting.key === 'offline.allowed')?.value !== false
  const on = write.isPending ? write.variables : effective.data ? offlineReadingOn(effective.data) : false

  return (
    <Section id="offline-reading-heading" title="Reading offline">
      <div data-part="offline-reading">
        {effective.isPending ? (
          <SettingsSkeleton label="Loading reading offline" sections={1} />
        ) : effective.isError ? (
          <ErrorState error={effective.error} onRetry={() => void effective.refetch()} title="This setting didn’t load" />
        ) : (
          <>
            <Switch
              checked={Boolean(on)}
              description={`Keep what you recently opened in ${name} on your devices, so it opens without a connection. Signing out clears it.`}
              disabled={!allowed || write.isPending}
              label="Read offline"
              onChange={(next) => write.mutate(next)}
            />
            {!allowed ? (
              <Banner role="note" tone="info">
                {name} doesn’t allow reading offline. Its admins can change that in the workspace’s General settings.
              </Banner>
            ) : null}
            {allowed && on && elsewhere.length ? (
              <Banner role="note" tone="info">
                Nothing is kept on this device while {elsewhere.map((workspace) => workspace.name).join(', ')}{' '}
                {elsewhere.length === 1 ? 'doesn’t' : 'don’t'} allow reading offline, because what it keeps covers every
                workspace you’re in.
              </Banner>
            ) : null}
            {write.isError ? <Banner tone="danger">It didn’t change. {describeError(write.error).message}</Banner> : null}
          </>
        )}
      </div>
    </Section>
  )
}
