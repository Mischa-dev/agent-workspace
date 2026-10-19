import { PageHeader } from '../../ui'
import { ScopedSettings } from '../ScopedSettings'
import type { SettingsPageProps } from '../types'

/** Workspace › Environments and run defaults: the `run.*` settings every project starts from. */
export default function RunDefaultsPage({ workspace }: SettingsPageProps) {
  if (!workspace) return null
  return (
    <>
      <PageHeader icon="play" title="Environments and run defaults">
        What new runs use unless a project or a single run says otherwise.
      </PageHeader>
      <ScopedSettings prefixes={['run.']} target={{ scope: 'workspace', organizationId: workspace.id }} />
    </>
  )
}
