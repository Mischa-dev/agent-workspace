import { useState } from 'react'
import { Link } from 'react-router-dom'

import { threadHref } from '../launch/launchModel'
import { Banner, Button, ErrorState, ListSkeleton, Section, Switch, showToast } from '../ui'
import type { AgentPresence, AgentProfile, ProfileTemplate } from './api'
import { AgentMark } from './AgentMark'
import { AgentProfileDialog } from './AgentProfileDialog'
import { usePresence, useProfiles, useUpdateProfile } from './hooks'
import { presenceLabel } from './presenceModel'
import './agents.css'

type Editing = { profile: AgentProfile | null; template: ProfileTemplate | null }

/** The project's own agents: made from a template or blank, each with its harness, model and instructions. */
export function AgentProfiles({ projectId }: { projectId: string }) {
  const profiles = useProfiles(projectId)
  const presence = usePresence(projectId)
  const [editing, setEditing] = useState<Editing | null>(null)
  const data = profiles.data
  const byProfile = new Map((presence.data?.agents ?? []).filter((agent) => agent.profileId).map((agent) => [agent.profileId, agent]))

  return (
    <Section
      description="Agents with their own name, harness, model and instructions. Mention one in a thread or pick it when you start a task."
      id="agent-profiles"
      title="Project agents"
    >
      {profiles.isPending ? <ListSkeleton label="Loading agents" rows={3} /> : null}
      {profiles.isError ? (
        <ErrorState action={<Button onClick={() => void profiles.refetch()}>Try again</Button>} title="Agents didn’t load">
          {profiles.error.message}
        </ErrorState>
      ) : null}
      {data && !data.ready ? (
        <Banner tone="info" title="Connect an account first.">
          Project agents run with the project’s default agent access. Connect an account in this project, then add agents here.
        </Banner>
      ) : null}
      {data && data.canManage && data.ready ? (
        <div className="aw-agentprofiles__templates" role="group" aria-label="Add an agent">
          {data.templates.map((template) => (
            <Button icon="plus" key={template.id} onClick={() => setEditing({ profile: null, template })}>
              {template.name}
            </Button>
          ))}
          <Button icon="plus" onClick={() => setEditing({ profile: null, template: null })} variant="ghost">
            Start blank
          </Button>
        </div>
      ) : null}
      {data && data.ready && data.profiles.length === 0 ? (
        <p className="aw-agentprofiles__quiet">
          {data.canManage ? 'No project agents yet. Start from a template above.' : 'No project agents yet. A project admin can add them.'}
        </p>
      ) : null}
      {data && data.profiles.length > 0 ? (
        <ul className="aw-agentprofiles__list">
          {data.profiles.map((profile) => (
            <ProfileCard
              canManage={data.canManage}
              key={profile.id}
              onEdit={() => setEditing({ profile, template: null })}
              presence={byProfile.get(profile.id)}
              profile={profile}
              projectId={projectId}
            />
          ))}
        </ul>
      ) : null}
      {editing && data ? (
        <AgentProfileDialog
          harnesses={data.harnesses}
          onClose={() => setEditing(null)}
          profile={editing.profile}
          projectId={projectId}
          template={editing.template}
        />
      ) : null}
    </Section>
  )
}

function ProfileCard({
  canManage,
  onEdit,
  presence,
  profile,
  projectId,
}: {
  canManage: boolean
  onEdit: () => void
  presence: AgentPresence | undefined
  profile: AgentProfile
  projectId: string
}) {
  const update = useUpdateProfile(projectId)
  const model = [profile.runDefaults.model, profile.runDefaults.reasoning, profile.runDefaults.speed].filter(Boolean).join(' · ')
  const toggle = (enabled: boolean) =>
    update.mutate(
      { profileId: profile.id, input: { enabled, version: profile.version } },
      {
        onSuccess: () => showToast({ tone: 'success', title: enabled ? `${profile.name} is on` : `${profile.name} is off` }),
        onError: (error) => showToast({ tone: 'danger', title: `${profile.name} wasn’t changed`, description: error.message }),
      },
    )
  return (
    <li className={profile.enabled ? 'aw-agentcard' : 'aw-agentcard is-off'}>
      <div className="aw-agentcard__head">
        <AgentMark avatar={profile.avatar} color={profile.color} name={profile.name} seed={profile.actorId} size={36} />
        <div className="aw-agentcard__name">
          <strong>{profile.name}</strong>
          <span className="aw-agentcard__meta">@{profile.handle}</span>
        </div>
        {canManage ? (
          <Button icon="pencil" onClick={onEdit} size="sm" variant="ghost">
            Edit
          </Button>
        ) : null}
      </div>
      {profile.description ? <p className="aw-agentcard__text">{profile.description}</p> : null}
      <p className="aw-agentcard__meta">
        {profile.harnessName ?? 'Project’s default harness'}
        {model ? ` · ${model}` : ''}
        {profile.environmentName ? ` · ${profile.environmentName}` : ''}
      </p>
      {profile.enabled ? <PresenceLine presence={presence} projectId={projectId} /> : <p className="aw-agentcard__meta">Off. Nobody can start it.</p>}
      {canManage ? (
        <Switch checked={profile.enabled} disabled={update.isPending} label="On" onChange={toggle} />
      ) : null}
    </li>
  )
}

function PresenceLine({ presence, projectId }: { presence: AgentPresence | undefined; projectId: string }) {
  const state = presence?.state ?? 'idle'
  const label = presence ? presenceLabel(presence) : 'Idle'
  return (
    <p className="aw-agentcard__presence">
      <span aria-hidden className={`aw-agentdot aw-agentdot--${state}`} />
      {presence?.conversationId ? <Link to={threadHref(projectId, presence.conversationId)}>{label}</Link> : label}
    </p>
  )
}
