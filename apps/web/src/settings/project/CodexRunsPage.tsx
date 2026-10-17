import { Link } from 'react-router-dom'

import { SponsoredExecutionSettings } from '../../external-agents/SponsoredExecutionSettings'
import { PageHeader, Section } from '../../ui'
import { accountSettingsHref } from '../paths'
import type { SettingsPageProps } from '../types'

/** Project › Codex runs: agents asking to run on your provider account in this Project. */
export default function CodexRunsPage({ project }: SettingsPageProps) {
  if (!project) return null
  return (
    <>
      <PageHeader icon="play" title="Codex runs">
        Agents can ask to run on your provider account. Nothing runs until you approve it here. Your sign-ins are under{' '}
        <Link to={`${accountSettingsHref('provider-accounts')}?project=${encodeURIComponent(project.id)}`}>Provider accounts</Link>.
      </PageHeader>
      <Section id="sponsored-execution-heading" title="Run requests from agents">
        <SponsoredExecutionSettings projectId={project.id} />
      </Section>
    </>
  )
}
