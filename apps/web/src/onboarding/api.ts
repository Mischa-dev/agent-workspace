import { requestJson } from '../api/client'

// The first-run path (P5). Each step is read on the server from what exists,
// so doing a step anywhere in the app ticks it here.

export type OnboardingStepId =
  | 'workspace'
  | 'project'
  | 'github'
  | 'repository'
  | 'account'
  | 'environment'
  | 'tasks'
  | 'agent'
  | 'launch'

export type OnboardingStep = { id: OnboardingStepId; done: boolean; skipped: boolean; optional: boolean }

export type Onboarding = {
  organizationId: string
  organizationName: string
  projectId: string | null
  projectName: string | null
  steps: OnboardingStep[]
  nextStep: OnboardingStepId | null
  complete: boolean
  dismissed: boolean
  canManageWorkspace: boolean
}

export type OnboardingChange = {
  projectId?: string
  confirm?: 'workspace'
  skip?: 'agent'
  unskip?: 'agent'
  dismissed?: boolean
}

export const onboardingKeys = {
  root: ['onboarding'] as const,
  workspace: (organizationId: string) => ['onboarding', organizationId] as const,
}

const path = (organizationId: string) => `/api/v1/organizations/${encodeURIComponent(organizationId)}/onboarding`

export const getOnboarding = (organizationId: string) => requestJson<Onboarding>(path(organizationId))

export const updateOnboarding = (organizationId: string, change: OnboardingChange) =>
  requestJson<Onboarding>(path(organizationId), { method: 'PUT', json: change })

export const STEP_TITLES: Record<OnboardingStepId, string> = {
  workspace: 'Name your workspace',
  project: 'Create a project',
  github: 'Connect GitHub',
  repository: 'Pick a repository',
  account: 'Connect an account for runs',
  environment: 'Choose an environment and defaults',
  tasks: 'Add your first tasks',
  agent: 'Add an agent',
  launch: 'Start your first run',
}

/** "4 of 8 done", counting skipped optional steps as out of the way. */
export function progressText(onboarding: Pick<Onboarding, 'steps'>) {
  const counted = onboarding.steps.filter((step) => !step.skipped)
  const done = counted.filter((step) => step.done).length
  return `${done} of ${counted.length} done`
}
