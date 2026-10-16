import { GitHubAutomationSettings } from '../../github/GitHubIntegrationSettings'
import { PageHeader } from '../../ui'
import type { SettingsPageProps } from '../types'

/** Project › Automation: what happens to work items when pull requests move. */
export default function AutomationPage({ project }: SettingsPageProps) {
  if (!project) return null
  return (
    <>
      <PageHeader icon="checklist" title="Automation">
        Keep work items in step with their pull requests on GitHub.
      </PageHeader>
      <GitHubAutomationSettings projectId={project.id} />
    </>
  )
}
