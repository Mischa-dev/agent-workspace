import { useSearchParams } from 'react-router-dom'

import { ExecutionAccountSettings } from '../../execution/ExecutionAccountSettings'
import { PageHeader, Section, SelectField } from '../../ui'
import type { SettingsPageProps } from '../types'

/** Account › Accounts for runs: the services your agents run on, signed in as you, and which one each project uses. */
export default function ProviderAccountsPage({ projects, session }: SettingsPageProps) {
  const [parameters, setParameters] = useSearchParams()
  const usable = projects.filter((project) => project.role !== 'viewer')
  const chosen = usable.find((project) => project.id === parameters.get('project')) ?? usable[0] ?? null

  return (
    <>
      <PageHeader icon="external" title="Accounts for runs">
        The services your agents run on, signed in as you. They’re yours alone: nobody else in a project can use them.
      </PageHeader>
      {chosen ? (
        <Section
          description="Choose which one each project’s runs use."
          id="codex-account-heading"
          title="Your accounts"
        >
          {usable.length > 1 && (
            <SelectField
              id="provider-account-project"
              label="Project"
              onChange={(event) => setParameters({ project: event.target.value }, { replace: true })}
              value={chosen.id}
            >
              {usable.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.name}
                </option>
              ))}
            </SelectField>
          )}
          <ExecutionAccountSettings actorId={session.user.actorId} key={chosen.id} projectId={chosen.id} />
        </Section>
      ) : (
        <p className="aw-quiet">Join a project as a member to connect an account for runs.</p>
      )}
    </>
  )
}
