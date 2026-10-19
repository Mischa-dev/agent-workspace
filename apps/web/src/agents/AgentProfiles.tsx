import { useState } from 'react'
import { Link } from 'react-router-dom'

import { toastError } from '../app/mutations'
import { threadHref } from '../launch/launchModel'
import { accountSettingsHref } from '../settings/paths'
import { Banner, Button, EmptyState, ErrorState, ListSkeleton, Section, Switch, showToast } from '../ui'
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
      {profiles.isError ? <ErrorState error={profiles.error} onRetry={() => void profiles.refetch()} title="Agents didn’t load" /> : null}
      {data && !data.ready ? (
        <Banner tone="info" title="Connect an account for runs first.">
          Project agents run on an account for runs connected to this project.{' '}
          <Link to={`${accountSettingsHref('provider-accounts')}?project=${encodeURIComponent(projectId)}`}>Connect one in Accounts for runs</Link>,
          then add agents here.
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
        <EmptyState icon="bot" title="No project agents yet">
          {data.canManage ? 'Start from a template above, or start blank.' : 'A project admin can add them.'}
        </EmptyState>
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
        onSuccess: (saved) =>
          showToast({
            title: enabled ? `${profile.name} enabled` : `${profile.name} disabled`,
            onUndo: () => update.mutate({ profileId: saved.id, input: { enabled: !enabled, version: saved.version } }),
          }),
        onError: (error) => toastError(enabled ? `${profile.name} wasn’t enabled` : `${profile.name} wasn’t disabled`, error),
      },
    )
  return (
    <li className={profile.enabled ? 'aw-profilecard' : 'aw-profilecard is-off'}>
      <div className="aw-profilecard__head">
        <AgentMark avatar={profile.avatar} color={profile.color} name={profile.name} seed={profile.actorId} size={36} />
        <div className="aw-profilecard__name">
          <strong>{profile.name}</strong>
          <span className="aw-profilecard__meta">@{profile.handle}</span>
        </div>
        {canManage ? (
          <Button icon="pencil" onClick={onEdit} size="sm" variant="ghost">
            Edit
          </Button>
        ) : null}
      </div>
      {profile.description ? <p className="aw-profilecard__text">{profile.description}</p> : null}
      <p className="aw-profilecard__meta">
        {profile.harnessName ?? 'Project’s default harness'}
        {model ? ` · ${model}` : ''}
        {profile.environmentName ? ` · ${profile.environmentName}` : ''}
      </p>
      {profile.enabled ? <PresenceLine presence={presence} projectId={projectId} /> : <p className="aw-profilecard__meta">Disabled. Nobody can start it.</p>}
      {canManage ? (
        <Switch checked={profile.enabled} disabled={update.isPending} label="Enabled" onChange={toggle} />
      ) : null}
    </li>
  )
}

function PresenceLine({ presence, projectId }: { presence: AgentPresence | undefined; projectId: string }) {
  const state = presence?.state ?? 'idle'
  const label = presence ? presenceLabel(presence) : 'Idle'
  return (
    <p className="aw-profilecard__presence">
      <span aria-hidden className={`aw-agentdot aw-agentdot--${state}`} />
      {presence?.conversationId ? <Link to={threadHref(projectId, presence.conversationId)}>{label}</Link> : label}
    </p>
  )
}
