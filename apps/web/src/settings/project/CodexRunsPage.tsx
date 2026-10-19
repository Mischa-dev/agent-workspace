import { Link } from 'react-router-dom'

import { SponsoredExecutionSettings } from '../../external-agents/SponsoredExecutionSettings'
import { PageHeader, Section } from '../../ui'
import { accountSettingsHref } from '../paths'
import type { SettingsPageProps } from '../types'

/** Project › Run requests: agents asking to run on your account in this project. */
export default function CodexRunsPage({ project }: SettingsPageProps) {
  if (!project) return null
  return (
    <>
      <PageHeader icon="circle-check" title="Run requests">
        Agents can ask to run on your account. Nothing runs until you approve it here. Your accounts are in{' '}
        <Link to={`${accountSettingsHref('provider-accounts')}?project=${encodeURIComponent(project.id)}`}>Accounts for runs</Link>.
      </PageHeader>
      <Section id="sponsored-execution-heading" title="Run requests from agents">
        <SponsoredExecutionSettings projectId={project.id} />
      </Section>
    </>
  )
}
