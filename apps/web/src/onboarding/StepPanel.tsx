import { Suspense, useState, type FormEvent, type ReactNode } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { queryKeys } from '../api/queryKeys'

import { aiKeys, getRanking } from '../ai/api'
import { toastError } from '../app/mutations'
import { useAuthenticatedSession } from '../auth/session'
import { QuickLaunch } from '../launch/QuickLaunch'
import { useCreateProject } from '../projects/hooks'
import { projectKeyPattern, suggestKey } from '../projects/projectKey'
import type { Project } from '../projects/types'
import { findPage } from '../settings/registry'
import type { SettingsGroup } from '../settings/types'
import { getWorkspace, renameWorkspace, workspaceKeys } from '../settings/workspace/api'
import { Banner, Button, ButtonLink, EmptyState, ListSkeleton, Row, RowList, SelectField, SettingsSkeleton, TextArea, TextField, describeError, showToast } from '../ui'
import { createWorkItem } from '../work-items/api'
import { onboardingKeys, STEP_TITLES, updateOnboarding, type Onboarding, type OnboardingChange, type OnboardingStepId } from './api'

type PanelProps = { onboarding: Onboarding; project: Project | null; projects: Project[] }
type GoTo = (step: OnboardingStepId) => void

const INTRO: Record<OnboardingStepId, string> = {
  workspace: 'This is the name everyone in the workspace sees. You can change it later in settings.',
  project: 'A project holds its own tasks, channels, repositories and agents. Agents start work from a project’s tasks.',
  github: 'Install the GitHub App so agents can read the code and open pull requests. This ticks as soon as GitHub reports the install.',
  repository: 'Pick the repository agents work in for this project.',
  account: 'Sign in to the service your agents run on, such as your coding assistant’s subscription. Each run uses the account its project points at.',
  environment: 'Choose where agents work and what each run starts with, such as the model and how fast it goes.',
  tasks: 'Write the first things you want done. A description and a few acceptance criteria make a task ready for an agent.',
  agent: 'Connect an agent of your own, such as one you run elsewhere, so it can pick up tasks too. You can skip this.',
  launch: 'Start an agent on a ready task. It works in the environment you chose, on the account you connected.',
}

/** One step of the first-run path, with what it needs right there. */
export function StepPanel({ onGoTo, step, ...props }: PanelProps & { onGoTo: GoTo; step: OnboardingStepId }) {
  const status = props.onboarding.steps.find((candidate) => candidate.id === step)
  const next = props.onboarding.nextStep
  return (
    <>
      <div className="aw-onboarding__panelhead">
        <h2 className="aw-onboarding__title" id="onboarding-step-title">
          {STEP_TITLES[step]}
        </h2>
        <p className="aw-onboarding__intro">{INTRO[step]}</p>
        {status?.done ? (
          <Banner
            action={
              next && next !== step ? (
                <Button iconEnd="chevron-right" onClick={() => onGoTo(next)} size="sm">
                  Next: {STEP_TITLES[next]}
                </Button>
              ) : null
            }
            tone="success"
          >
            Done.{next ? '' : ' That was the last step.'}
          </Banner>
        ) : null}
      </div>
      <Body onGoTo={onGoTo} step={step} {...props} />
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

function Body({ onGoTo, onboarding, project, projects, step }: PanelProps & { onGoTo: GoTo; step: OnboardingStepId }) {
  const needsProject = !['workspace', 'project', 'account'].includes(step)
  if (needsProject && !project) {
    return (
      <EmptyState
        action={
          <Button onClick={() => onGoTo('project')} variant="primary">
            Choose a project
          </Button>
        }
        icon="home"
        title="Choose a project first"
      >
        This step belongs to a project. Create or pick one first.
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
      return <Embedded group="project" id="environments" onboarding={onboarding} project={project} projects={projects} />
    case 'tasks':
      return <TasksStep onboarding={onboarding} project={project as Project} />
    case 'agent':
      return <AgentStep onboarding={onboarding} project={project} projects={projects} />
    case 'launch':
      return <LaunchStep onGoTo={onGoTo} project={project as Project} />
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

/** Just the name: confirming it (changed or not) is what ticks the step. */
function WorkspaceStep({ onboarding }: Omit<PanelProps, 'project'>) {
  const queryClient = useQueryClient()
  const change = useChange(onboarding)
  const organizationId = onboarding.organizationId
  const workspace = useQuery({
    queryKey: workspaceKeys.workspace(organizationId),
    queryFn: () => getWorkspace(organizationId),
    enabled: onboarding.canManageWorkspace,
  })
  const [typed, setTyped] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const current = workspace.data?.name ?? onboarding.organizationName
  const name = typed ?? current
  const renamed = name.trim() !== current
  const rename = useMutation({
    mutationFn: (version: number) => renameWorkspace(organizationId, name.trim(), version),
    onSuccess: (updated) => {
      queryClient.setQueryData(workspaceKeys.workspace(organizationId), updated)
      void queryClient.invalidateQueries({ queryKey: queryKeys.session })
      setTyped(null)
      change.mutate({ confirm: 'workspace' })
    },
    onError: (reason) => setError(describeError(reason).message),
  })
  if (!onboarding.canManageWorkspace) {
    return <p className="aw-onboarding__intro">An owner or admin names the workspace. You’re set to go on.</p>
  }
  if (workspace.isPending) return <SettingsSkeleton label="Loading the workspace" sections={1} />
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!name.trim()) {
      setError('Give the workspace a name.')
      return
    }
    setError(null)
    if (renamed && workspace.data) rename.mutate(workspace.data.version)
    else change.mutate({ confirm: 'workspace' })
  }
  return (
    <form className="aw-form aw-onboarding__form" noValidate onSubmit={submit}>
      <TextField
        error={error ?? undefined}
        id="onboarding-workspace-name"
        label="Workspace name"
        maxLength={120}
        onChange={(event) => setTyped(event.target.value)}
        value={name}
      />
      <div className="aw-onboarding__actions">
        <Button busy={rename.isPending || change.isPending} type="submit" variant="primary">
          {renamed ? 'Save and continue' : 'Use this name'}
        </Button>
      </div>
    </form>
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
        <p className="aw-onboarding__intro">Ask a workspace owner or admin to create a project and add you to it.</p>
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
        <TextArea
          hint="What should be done, and anything an agent should know."
          id="onboarding-task-description"
          label="Description"
          onChange={(event) => setDescription(event.target.value)}
          rows={3}
          value={description}
        />
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
          <ButtonLink to={`/projects/${encodeURIComponent(project.id)}/work-items`} variant="ghost">
            Open {project.name}’s tasks
          </ButtonLink>
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

function LaunchStep({ onGoTo, project }: { onGoTo: GoTo; project: Project }) {
  const filters = { scope: 'agents' as const, limit: 5 }
  const ranking = useQuery({ queryKey: aiKeys.ranking(project.id, filters), queryFn: () => getRanking(project.id, filters) })
  if (ranking.isPending) return <ListSkeleton label="Finding ready tasks" rows={3} />
  if (ranking.isError) {
    return (
      <Banner onRetry={() => void ranking.refetch()} tone="danger">
        Ready tasks didn’t load.
      </Banner>
    )
  }
  if (ranking.data.tasks.length === 0) {
    return (
      <EmptyState
        action={
          <Button onClick={() => onGoTo('tasks')} variant="primary">
            Add a task
          </Button>
        }
        icon="checklist"
        title="No task is ready for an agent yet"
      >
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
