import { useSearchParams } from 'react-router-dom'

import { ExecutionAccountSettings } from '../../execution/ExecutionAccountSettings'
import { PageHeader, Section, SelectField } from '../../ui'
import type { SettingsPageProps } from '../types'

/** Account › Provider accounts: the Codex sign-in your runs use, per Project for now. */
export default function ProviderAccountsPage({ projects, session }: SettingsPageProps) {
  const [parameters, setParameters] = useSearchParams()
  const usable = projects.filter((project) => project.role !== 'viewer')
  const chosen = usable.find((project) => project.id === parameters.get('project')) ?? usable[0] ?? null

  return (
    <>
      <PageHeader icon="terminal" title="Provider accounts">
        The accounts your runs use. They’re yours alone: nobody else in a Project can use them.
      </PageHeader>
      {chosen ? (
        <Section
          description="Today each Project has its own Codex sign-in. Pick the Project to see or change it."
          id="codex-account-heading"
          title="Codex account"
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
        <p className="aw-quiet">Join a Project as a member to connect an account for runs.</p>
      )}
    </>
  )
}
