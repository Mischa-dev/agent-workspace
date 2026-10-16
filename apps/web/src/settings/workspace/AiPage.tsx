import { PageHeader } from '../../ui'
import { ScopedSettings } from '../ScopedSettings'
import type { SettingsPageProps } from '../types'

/** Workspace › AI and models: the `ai.*` settings for the whole workspace. */
export default function AiPage({ workspace }: SettingsPageProps) {
  if (!workspace) return null
  return (
    <>
      <PageHeader icon="bot" title="AI and models">
        Which provider and model the server’s short jobs use, such as finding dependencies between work items. Projects can
        override these.
      </PageHeader>
      <ScopedSettings prefixes={['ai.']} target={{ scope: 'workspace', organizationId: workspace.id }} />
    </>
  )
}
