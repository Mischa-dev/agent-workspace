import { Suspense, lazy, useState, type FormEvent, type ReactNode } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { aiKeys, getRanking } from '../ai/api'
import { toastError } from '../app/mutations'
import { useAuthenticatedSession } from '../auth/session'
import { QuickLaunch } from '../launch/QuickLaunch'
import { useCreateProject } from '../projects/hooks'
import { projectKeyPattern, suggestKey } from '../projects/projectKey'
import type { Project } from '../projects/types'
import { findPage } from '../settings/registry'
import type { SettingsGroup } from '../settings/types'
import { Banner, Button, EmptyState, ListSkeleton, Row, RowList, SelectField, SettingsSkeleton, TextArea, TextField, showToast } from '../ui'
import { createWorkItem } from '../work-items/api'
import { onboardingKeys, STEP_TITLES, updateOnboarding, type Onboarding, type OnboardingChange, type OnboardingStepId } from './api'

type PanelProps = { onboarding: Onboarding; project: Project | null; projects: Project[] }

const INTRO: Record<OnboardingStepId, string> = {
  workspace: 'This is the name everyone in the workspace sees. You can change it later in settings.',
  project: 'A project holds its own tasks, channels, repositories and agents. Runs launch from a project’s tasks.',
  github: 'Install the GitHub App so agents can read the code and open pull requests. It ticks as soon as GitHub reports the install.',
  repository: 'Pick the repository agents work in for this project.',
  account: 'Sign in a provider account to run agents on. Any harness works: each run uses the account its project points at.',
  environment: 'Choose where runs happen and the defaults they start with: harness, model, reasoning and speed.',
  tasks: 'Write the first things you want done. A description and a few acceptance criteria make a task ready for an agent.',
  agent: 'Connect an agent of your own, such as a harness you run elsewhere, so it can pick up tasks too. You can skip this.',
  launch: 'Start an agent on a ready task. It runs in the environment you chose, on the provider account you connected.',
}

/** One step of the first-run path, with what it needs right there. */
export function StepPanel({ step, ...props }: PanelProps & { step: OnboardingStepId }) {
  const status = props.onboarding.steps.find((candidate) => candidate.id === step)
  return (
    <>
      <div className="aw-onboarding__panelhead">
        <h2 className="aw-onboarding__title" id="onboarding-step-title">
          {STEP_TITLES[step]}
        </h2>
        <p className="aw-onboarding__intro">{INTRO[step]}</p>
        {status?.done ? <Banner tone="success">Done. Go on to the next step whenever you’re ready.</Banner> : null}
      </div>
      <Body step={step} {...props} />
    </>
  )
}

function useChange(onboarding: Onboarding) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (change: OnboardingChange) => updateOnboarding(onboarding.organizationId, change),
    onSuccess: (updated) => queryClient.setQueryData(onboardingKeys.workspace(onboarding.organizationId), updated),
    onError: (reason) => toastError('Setup didn’t save', reason),
  })
}

function Body({ onboarding, project, projects, step }: PanelProps & { step: OnboardingStepId }) {
  const needsProject = !['workspace', 'project', 'account'].includes(step)
  if (needsProject && !project) {
    return (
      <EmptyState icon="home" title="Choose a project first">
        This step belongs to a project. Create or pick one in the project step.
      </EmptyState>
    )
  }
  switch (step) {
    case 'workspace':
      return <WorkspaceStep onboarding={onboarding} projects={projects} />
    case 'project':
      return <ProjectStep onboarding={onboarding} project={project} projects={projects} />
    case 'github':
    case 'repository':
      return <Embedded group="project" id="repositories" onboarding={onboarding} project={project} projects={projects} />
    case 'account':
      return <Embedded group="account" id="provider-accounts" onboarding={onboarding} project={project} projects={projects} />
    case 'environment':
      return <RunSetupStep onboarding={onboarding} project={project} projects={projects} />
    case 'tasks':
      return <TasksStep onboarding={onboarding} project={project as Project} />
    case 'agent':
      return <AgentStep onboarding={onboarding} project={project} projects={projects} />
    case 'launch':
      return <LaunchStep project={project as Project} />
  }
}

/** A settings page, shown in the path so nobody has to leave it. */
function Embedded({ group, id, onboarding, project, projects, after }: PanelProps & { group: SettingsGroup; id: string; after?: ReactNode }) {
  const session = useAuthenticatedSession()
  const page = findPage(group, id)
  const workspace = session.organizations.find((organization) => organization.id === onboarding.organizationId) ?? null
  if (!page) return null
  const Page = page.component
  return (
    <div className="aw-onboarding__embed">
      <Suspense fallback={<SettingsSkeleton label={`Loading ${page.label.toLowerCase()}`} sections={2} />}>
        <Page project={group === 'project' ? project : null} projects={projects} session={session} workspace={workspace} />
      </Suspense>
      {after}
    </div>
  )
}

const RunSetup = lazy(() => import('../settings/project/RunsPage').then((module) => ({ default: module.RunSetup })))

/** Where runs work and the defaults they start with: the setup part of the project's Runs and models settings. */
function RunSetupStep({ onboarding, project, projects }: PanelProps) {
  const session = useAuthenticatedSession()
  const workspace = session.organizations.find((organization) => organization.id === onboarding.organizationId) ?? null
  return (
    <div className="aw-onboarding__embed">
      <Suspense fallback={<SettingsSkeleton label="Loading run settings" sections={2} />}>
        <RunSetup project={project} projects={projects} session={session} workspace={workspace} />
      </Suspense>
    </div>
  )
}

function WorkspaceStep({ onboarding, projects }: Omit<PanelProps, 'project'>) {
  const change = useChange(onboarding)
  if (!onboarding.canManageWorkspace) {
    return <p className="aw-onboarding__intro">An owner or admin names the workspace. You’re set to go on.</p>
  }
  return (
    <Embedded
      after={
        <div className="aw-onboarding__actions">
          <Button busy={change.isPending} onClick={() => change.mutate({ confirm: 'workspace' })} variant="primary">
            The name is right
          </Button>
        </div>
      }
      group="workspace"
      id="general"
      onboarding={onboarding}
      project={null}
      projects={projects}
    />
  )
}

function ProjectStep({ onboarding, project, projects }: PanelProps) {
  const session = useAuthenticatedSession()
  const change = useChange(onboarding)
  const create = useCreateProject()
  const inWorkspace = projects.filter((candidate) => candidate.organizationId === onboarding.organizationId && !candidate.archivedAt)
  const [name, setName] = useState('')
  const [idempotencyKey, setIdempotencyKey] = useState(() => crypto.randomUUID())
  const [error, setError] = useState<string | null>(null)
  const key = suggestKey(name)
  const canCreate = session.user.capabilities.createProject && onboarding.canManageWorkspace

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim() || !projectKeyPattern.test(key)) {
      setError('Give the project a name of at least two letters.')
      return
    }
    setError(null)
    try {
      const created = await create.mutateAsync({ organizationId: onboarding.organizationId, key, name: name.trim(), idempotencyKey })
      setName('')
      setIdempotencyKey(crypto.randomUUID())
      change.mutate({ projectId: created.id })
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'The project wasn’t created. Try again.')
    }
  }

  return (
    <div className="aw-onboarding__stack">
      {inWorkspace.length > 0 ? (
        <SelectField
          hint="The rest of setup happens in this project."
          id="onboarding-project"
          label="Project to set up"
          onChange={(event) => change.mutate({ projectId: event.target.value })}
          value={project?.id ?? ''}
        >
          {!project ? <option value="">Choose a project</option> : null}
          {inWorkspace.map((candidate) => (
            <option key={candidate.id} value={candidate.id}>
              {candidate.name}
            </option>
          ))}
        </SelectField>
      ) : null}
      {canCreate ? (
        <form className="aw-form aw-onboarding__form" noValidate onSubmit={submit}>
          <TextField
            error={error ?? undefined}
            hint={key && projectKeyPattern.test(key) ? `Tasks will be numbered ${key}-1, ${key}-2 and so on.` : 'For example, the product or repository it’s about.'}
            id="onboarding-project-name"
            label={inWorkspace.length ? 'Or create a new project' : 'Project name'}
            maxLength={120}
            onChange={(event) => setName(event.target.value)}
            placeholder="e.g. Trident"
            value={name}
          />
          <div className="aw-onboarding__actions">
            <Button busy={create.isPending} type="submit" variant={inWorkspace.length ? 'secondary' : 'primary'}>
              Create project
            </Button>
          </div>
        </form>
      ) : inWorkspace.length === 0 ? (
        <p className="aw-onboarding__intro">Ask an owner or admin to create a project and invite you.</p>
      ) : null}
    </div>
  )
}

function TasksStep({ onboarding, project }: { onboarding: Onboarding; project: Project }) {
  const queryClient = useQueryClient()
  const [title, setTitle] = useState('')
  const [description, setDescription] = useState('')
  const [criteria, setCriteria] = useState('')
  const [idempotencyKey, setIdempotencyKey] = useState(() => crypto.randomUUID())
  const [added, setAdded] = useState<{ id: string; key: string; title: string }[]>([])
  const create = useMutation({
    mutationFn: () =>
      createWorkItem({
        projectId: project.id,
        title: title.trim(),
        description: description.trim() || undefined,
        priority: 'no_priority',
        acceptanceCriteria: criteria
          .split('\n')
          .map((line) => line.replace(/^[-*]\s*/, '').trim())
          .filter(Boolean),
        idempotencyKey,
      }),
    onSuccess: (item) => {
      setAdded((current) => [...current, { id: item.id, key: item.key, title: item.title }])
      setTitle('')
      setDescription('')
      setCriteria('')
      setIdempotencyKey(crypto.randomUUID())
      showToast({ tone: 'success', title: `${item.key} added` })
      void queryClient.invalidateQueries({ queryKey: onboardingKeys.workspace(onboarding.organizationId) })
      void queryClient.invalidateQueries({ queryKey: aiKeys.root(project.id) })
    },
    onError: (reason) => toastError('The task wasn’t added', reason),
  })
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (title.trim()) create.mutate()
  }
  return (
    <div className="aw-onboarding__stack">
      <form className="aw-form aw-onboarding__form" noValidate onSubmit={submit}>
        <TextField id="onboarding-task-title" label="Title" maxLength={300} onChange={(event) => setTitle(event.target.value)} placeholder="e.g. Add a dark theme" required value={title} />
        <TextArea id="onboarding-task-description" label="What should be done" onChange={(event) => setDescription(event.target.value)} rows={3} value={description} />
        <TextArea
          hint="One per line. An agent checks its work against these."
          id="onboarding-task-criteria"
          label="Acceptance criteria"
          onChange={(event) => setCriteria(event.target.value)}
          rows={3}
          value={criteria}
        />
        <div className="aw-onboarding__actions">
          <Button busy={create.isPending} disabled={!title.trim()} type="submit" variant="primary">
            Add task
          </Button>
          <Link className="aw-btn aw-btn--ghost aw-btn--md" to={`/projects/${encodeURIComponent(project.id)}/work-items`}>
            <span className="aw-btn__label">Open {project.name}’s tasks</span>
          </Link>
        </div>
      </form>
      {added.length > 0 ? (
        <RowList label="Added here">
          {added.map((item) => (
            <Row
              key={item.id}
              title={
                <Link className="aw-row__link" to={`/projects/${encodeURIComponent(project.id)}/work-items/${encodeURIComponent(item.id)}`}>
                  {item.key} {item.title}
                </Link>
              }
            />
          ))}
        </RowList>
      ) : null}
    </div>
  )
}

function AgentStep(props: PanelProps) {
  const change = useChange(props.onboarding)
  const skipped = props.onboarding.steps.find((step) => step.id === 'agent')?.skipped ?? false
  if (!props.project?.capabilities.manage) {
    return (
      <div className="aw-onboarding__actions">
        <p className="aw-onboarding__intro">A project admin adds agents.</p>
        <Button busy={change.isPending} onClick={() => change.mutate({ skip: 'agent' })} variant="secondary">
          Skip for now
        </Button>
      </div>
    )
  }
  return (
    <Embedded
      {...props}
      after={
        <div className="aw-onboarding__actions">
          {skipped ? (
            <Button busy={change.isPending} onClick={() => change.mutate({ unskip: 'agent' })} variant="ghost">
              Don’t skip this
            </Button>
          ) : (
            <Button busy={change.isPending} onClick={() => change.mutate({ skip: 'agent' })} variant="secondary">
              Skip for now
            </Button>
          )}
        </div>
      }
      group="project"
      id="agents"
    />
  )
}

function LaunchStep({ project }: { project: Project }) {
  const filters = { scope: 'agents' as const, limit: 5 }
  const ranking = useQuery({ queryKey: aiKeys.ranking(project.id, filters), queryFn: () => getRanking(project.id, filters) })
  if (ranking.isPending) return <ListSkeleton label="Finding ready tasks" rows={3} />
  if (ranking.isError) {
    return (
      <Banner tone="danger">
        Ready tasks didn’t load.{' '}
        <Button onClick={() => void ranking.refetch()} size="sm" variant="ghost">
          Try again
        </Button>
      </Banner>
    )
  }
  if (ranking.data.tasks.length === 0) {
    return (
      <EmptyState icon="checklist" title="No task is ready for an agent yet">
        Give a task a description and acceptance criteria, and make sure the project has a repository. It shows here once it’s ready.
      </EmptyState>
    )
  }
  return (
    <RowList label="Ready for an agent">
      {ranking.data.tasks.map((task) => {
        const missing = task.readiness.checks.filter((check) => !check.ok && check.forAgents)
        return (
          <Row
            actions={task.readiness.readyForAgents ? <QuickLaunch task={{ id: task.id, key: task.key, projectId: project.id }} /> : null}
            key={task.id}
            meta={missing.length ? `Needs: ${missing.map((check) => check.label.toLowerCase()).join(', ')}` : task.reasons.slice(0, 2).join(' · ')}
            title={
              <Link className="aw-row__link" to={task.href}>
                {task.key} {task.title}
              </Link>
            }
          />
        )
      })}
    </RowList>
  )
}
