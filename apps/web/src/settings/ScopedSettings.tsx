import { useEffect } from 'react'
import { useLocation } from 'react-router-dom'

import { Banner, ErrorState, SettingsSkeleton } from '../ui'
import { useEffectiveSettings, useSettingsRegistry, type SettingOption, type SettingsTarget } from './api'
import { SettingField } from './SettingField'

/**
 * The registry settings whose keys start with one of `prefixes`, as they apply
 * at `target`. New keys in the registry show up here without a code change.
 */
export function ScopedSettings({
  emptyText = 'There’s nothing to set here yet.',
  options,
  prefixes,
  target,
}: {
  emptyText?: string
  options?: Record<string, SettingOption[]>
  prefixes: string[]
  target: SettingsTarget
}) {
  const registry = useSettingsRegistry()
  const effective = useEffectiveSettings(target)
  const location = useLocation()
  const ready = registry.isSuccess && effective.isSuccess

  // Search links straight to one setting: bring it into view.
  useEffect(() => {
    if (!ready || !location.hash.startsWith('#setting-')) return
    const element = document.getElementById(decodeURIComponent(location.hash.slice(1)))
    if (!element) return
    element.scrollIntoView?.({ block: 'center' })
    element.focus({ preventScroll: true })
    element.classList.add('is-found')
    const timer = window.setTimeout(() => element.classList.remove('is-found'), 2000)
    return () => window.clearTimeout(timer)
  }, [location.hash, ready])

  if (registry.isPending || effective.isPending) return <SettingsSkeleton label="Loading settings" />
  if (registry.isError || effective.isError) {
    return (
      <ErrorState
        error={registry.error ?? effective.error}
        onRetry={() => {
          void registry.refetch()
          void effective.refetch()
        }}
        title="Settings didn’t load"
      />
    )
  }

  const definitions = registry.data.filter(
    (definition) =>
      definition.scopes.includes(target.scope) && prefixes.some((prefix) => definition.key.startsWith(prefix)),
  )
  const canEdit =
    target.scope === 'project' ? effective.data.capabilities.manageProject : effective.data.capabilities.manageWorkspace

  return (
    <div className="aw-settings-stack">
      {!canEdit && (
        <Banner role="note" tone="info">
          {target.scope === 'project'
            ? 'Only project owners and admins can change these.'
            : 'Only workspace owners and admins can change these.'}
        </Banner>
      )}
      {definitions.length ? (
        <div className="aw-setting-list">
          {definitions.map((definition) => {
            const resolved = effective.data.settings.find((setting) => setting.key === definition.key)
            return resolved ? (
              <SettingField
                canEdit={canEdit}
                definition={definition}
                key={definition.key}
                options={options?.[definition.key]}
                resolved={resolved}
                target={target}
              />
            ) : null
          })}
        </div>
      ) : (
        <p className="aw-quiet">{emptyText}</p>
      )}
    </div>
  )
}
