import { PageHeader } from '../../ui'
import { ScopedSettings } from '../ScopedSettings'
import type { SettingsPageProps } from '../types'

/** Workspace › Environments and run defaults: the `run.*` settings every Project starts from. */
export default function RunDefaultsPage({ workspace }: SettingsPageProps) {
  if (!workspace) return null
  return (
    <>
      <PageHeader icon="settings" title="Environments and run defaults">
        What new runs use unless a Project or a single run says otherwise.
      </PageHeader>
      <ScopedSettings prefixes={['run.']} target={{ scope: 'workspace', organizationId: workspace.id }} />
    </>
  )
}
