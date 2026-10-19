import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import type { OrganizationSummary } from '../../auth/session'
import { listWorkspaceHarnesses, modelCatalogKey } from '../../execution/api'
import type { Project } from '../../projects/types'
import { Banner, PageHeader, Section } from '../../ui'
import { useEffectiveSettings, type SettingsTarget } from '../api'
import { accountSettingsHref } from '../paths'
import { LaunchPrompt } from '../runs/LaunchPrompt'
import { WorkspaceModelLists } from '../runs/ModelLists'
import { RUN_UNSET_LABELS, harnessAt, runSettingOptions } from '../runs/options'
import { useSectionScroll } from '../runs/useSectionScroll'
import { WorkspaceEnvironments } from '../runs/WorkspaceEnvironments'
import { WorkspaceSuggestions } from '../runs/WorkspaceSuggestions'
import { ScopedSettings } from '../ScopedSettings'
import type { SettingsPageProps } from '../types'

/**
 * Workspace › Runs and models: what new runs use unless a project or the run
 * says otherwise, each project's environments, the models people can pick,
 * what suggestions and summaries run on, and the launch prompt.
 */
export default function RunsPage({ projects, workspace }: SettingsPageProps) {
  return workspace ? <WorkspaceRuns projects={projects} workspace={workspace} /> : null
}

function WorkspaceRuns({ projects, workspace }: { projects: Project[]; workspace: OrganizationSummary }) {
  const target: SettingsTarget = { scope: 'workspace', organizationId: workspace.id }
  const effective = useEffectiveSettings(target)
  const harnesses = useQuery({ queryKey: modelCatalogKey(workspace.id), queryFn: () => listWorkspaceHarnesses(workspace.id) })
  useSectionScroll(effective.isSuccess && !harnesses.isPending)
  const manager = workspace.role === 'owner' || workspace.role === 'admin'
  const readOnly = effective.data ? !effective.data.capabilities.manageWorkspace : false

  return (
    <>
      <PageHeader icon="activity" title="Runs and models">
        What runs and agents in {workspace.name} use, and the models people can pick. Each project can change any of it.
      </PageHeader>
      {readOnly ? (
        <Banner role="note" tone="info">
          Only workspace owners and admins can change these.
        </Banner>
      ) : null}
      <Section
        description={
          <>
            New runs use these unless the launch, the agent or the project picks something else. Where nothing is set here,
            the defaults of the <Link to={accountSettingsHref('provider-accounts')}>provider account</Link> that runs it apply.
          </>
        }
        id="defaults"
        title="Defaults"
      >
        <ScopedSettings
          options={runSettingOptions(harnesses.data ?? [], harnessAt(effective.data, target))}
          prefixes={['run.']}
          readOnlyNote={false}
          target={target}
          unsetLabels={RUN_UNSET_LABELS}
        />
      </Section>
      <Section
        description="Environments are set up in each project, with its repository. Without one, runs work on this server."
        id="environments"
        title="Environments"
      >
        <WorkspaceEnvironments projects={projects.filter((project) => project.organizationId === workspace.id && !project.archivedAt)} />
      </Section>
      <Section description={`Everyone in ${workspace.name} picks from these lists, in every project.`} id="models" title="Models">
        <WorkspaceModelLists organizationId={workspace.id} />
      </Section>
      <WorkspaceSuggestions manager={manager} organizationId={workspace.id} />
      <Section description="The prompt Launch writes from a task, in every project that doesn’t have its own." id="launch-prompt" title="Launch prompt">
        <LaunchPrompt target={target} />
      </Section>
    </>
  )
}
