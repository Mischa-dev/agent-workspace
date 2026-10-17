import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { useAuthenticatedSession } from '../auth/session'
import { useProjects } from '../projects/hooks'
import { defaultWorkspace } from '../settings/workspaceChoice'
import { AppShell } from '../shell/AppShell'
import { Button, EmptyState, ErrorState, Icon, PageHeader, SettingsSkeleton } from '../ui'
import { getOnboarding, onboardingKeys, progressText, STEP_TITLES, type Onboarding, type OnboardingStepId } from './api'
import { StepPanel } from './StepPanel'
import '../settings/settings.css'
import './onboarding.css'

/**
 * /welcome: the first-run path (P5). A new workspace goes from its name to
 * its first launched run here: each step shows the settings it needs right
 * in the path, and ticks itself when the server sees it done, wherever it
 * was done.
 */
export function OnboardingPage() {
  const session = useAuthenticatedSession()
  const workspace = defaultWorkspace(session)
  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page aw-onboarding">
        {workspace ? (
          <OnboardingFor organizationId={workspace.id} />
        ) : (
          <EmptyState icon="home" title="No workspace yet">
            Ask an owner to invite you to their workspace, or set one up from the command line.
          </EmptyState>
        )}
      </div>
    </AppShell>
  )
}

function OnboardingFor({ organizationId }: { organizationId: string }) {
  // Steps done elsewhere (a GitHub install, a sign-in in another tab) tick here soon after.
  const onboarding = useQuery({
    queryKey: onboardingKeys.workspace(organizationId),
    queryFn: () => getOnboarding(organizationId),
    refetchInterval: 10_000,
    refetchOnWindowFocus: true,
  })
  if (onboarding.isPending) return <SettingsSkeleton label="Loading setup" sections={2} />
  if (onboarding.isError) {
    return (
      <ErrorState action={<Button onClick={() => void onboarding.refetch()}>Try again</Button>} title="Setup didn’t load">
        Check your connection and try again.
      </ErrorState>
    )
  }
  return <Path onboarding={onboarding.data} />
}

function Path({ onboarding }: { onboarding: Onboarding }) {
  const projects = useProjects()
  // A step picked while still to do gives way to the next one once it's done.
  const [chosen, setChosen] = useState<{ id: OnboardingStepId; wasDone: boolean } | null>(null)
  const isDone = (id: OnboardingStepId) => onboarding.steps.find((step) => step.id === id)?.done ?? false
  const active = chosen && (chosen.wasDone || !isDone(chosen.id)) ? chosen.id : (onboarding.nextStep ?? 'launch')
  const project = (projects.data?.projects ?? []).find((candidate) => candidate.id === onboarding.projectId) ?? null

  return (
    <>
      <PageHeader
        actions={
          <Link className="aw-btn aw-btn--ghost aw-btn--md" to="/home">
            <span className="aw-btn__label">{onboarding.complete ? 'Go to Home' : 'Finish later'}</span>
          </Link>
        }
        icon="home"
        title={onboarding.complete ? `${onboarding.organizationName} is set up` : `Set up ${onboarding.organizationName}`}
      >
        {onboarding.complete
          ? 'Everything’s in place. Launch more from any task, or from the usage page before a plan resets.'
          : `From a name to your first launched run. ${progressText(onboarding)}.`}
      </PageHeader>
      <div className="aw-onboarding__layout">
        <nav aria-label="Setup steps" className="aw-onboarding__steps">
          <ol>
            {onboarding.steps.map((step, index) => {
              const state = step.done ? 'done' : step.skipped ? 'skipped' : step.id === onboarding.nextStep ? 'next' : 'todo'
              return (
                <li key={step.id}>
                  <button
                    aria-current={step.id === active ? 'step' : undefined}
                    className={`aw-onboarding__step is-${state}`}
                    onClick={() => setChosen({ id: step.id, wasDone: step.done })}
                    type="button"
                  >
                    <span aria-hidden="true" className="aw-onboarding__mark">
                      {step.done ? <Icon name="check" size={14} /> : index + 1}
                    </span>
                    <span className="aw-onboarding__steptitle">{STEP_TITLES[step.id]}</span>
                    <span className="aw-sr-only">
                      {step.done ? ', done' : step.skipped ? ', skipped' : step.optional ? ', optional' : ''}
                    </span>
                    {step.optional && !step.done ? <span className="aw-onboarding__optional">Optional</span> : null}
                  </button>
                </li>
              )
            })}
          </ol>
        </nav>
        <section aria-labelledby="onboarding-step-title" className="aw-onboarding__panel">
          <StepPanel onboarding={onboarding} project={project} projects={projects.data?.projects ?? []} step={active} />
        </section>
      </div>
    </>
  )
}
