import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { requestJson } from '../../api/client'
import { queryKeys } from '../../api/queryKeys'
import { describeError } from '../../app/mutations'
import { startWorkOs, workOsStatus } from '../../auth/workos'
import { Avatar, Banner, Button, PageHeader, Section, SettingsSkeleton, TextField, describeLoadError, showToast } from '../../ui'
import type { SettingsPageProps } from '../types'
import { EmailSection } from './EmailSection'

/** Account › Profile: your name, your email address, your GitHub username and how you sign in. */
export default function ProfilePage({ session, workspace }: SettingsPageProps) {
  const client = useQueryClient()
  const [name, setName] = useState(session.user.displayName)
  const [error, setError] = useState<string | null>(null)
  const status = useQuery({ queryKey: ['workos-status'], queryFn: workOsStatus })
  const profile = useQuery({
    queryKey: ['account-profile'],
    queryFn: () => requestJson<{ githubLogin?: string | null }>('/api/v1/account/profile'),
  })
  const [githubDraft, setGithubDraft] = useState<string | null>(null)
  const githubLogin = githubDraft ?? profile.data?.githubLogin ?? ''
  const saveGithub = useMutation({
    mutationFn: () => requestJson('/api/v1/account/profile', { method: 'PATCH', json: { githubLogin: githubLogin.trim() } }),
    onSuccess: async () => {
      setError(null)
      setGithubDraft(null)
      showToast({ tone: 'success', title: githubLogin.trim() ? 'GitHub username saved' : 'GitHub username removed' })
      await client.invalidateQueries({ queryKey: ['account-profile'] })
    },
    onError: (reason) => setError(`Your GitHub username wasn’t saved. ${describeError(reason).message}`),
  })
  const save = useMutation({
    mutationFn: () => requestJson('/api/v1/account/profile', { method: 'PATCH', json: { displayName: name.trim() } }),
    onSuccess: async () => {
      setError(null)
      showToast({ tone: 'success', title: 'Display name saved' })
      await client.invalidateQueries({ queryKey: queryKeys.session })
    },
    onError: (reason) => setError(`Your display name wasn’t saved. ${describeError(reason).message}`),
  })
  const link = useMutation({
    mutationFn: (provider?: 'GoogleOAuth') => startWorkOs({ link: true, provider }),
    onError: () => setError('Single sign-on didn’t open. Try again.'),
  })
  const submit = (event: FormEvent) => {
    event.preventDefault()
    save.mutate()
  }

  return (
    <>
      <PageHeader
        mark={<Avatar name={session.user.displayName} seed={session.user.actorId} size={40} />}
        title="Profile"
      >
        Your identity stays with you across workspaces and projects.
      </PageHeader>
      {error && <Banner tone="danger">{error}</Banner>}
      <Section id="profile-heading" title="Name">
        <form className="aw-inline-form" onSubmit={submit}>
          <TextField id="account-username" label="Username" mono readOnly value={session.user.loginName} />
          <TextField
            id="account-name"
            label="Display name"
            maxLength={120}
            onChange={(event) => setName(event.target.value)}
            required
            value={name}
          />
          <Button busy={save.isPending} disabled={!name.trim() || name.trim() === session.user.displayName} type="submit" variant="primary">
            Save
          </Button>
        </form>
      </Section>
      <EmailSection canSetUpEmail={workspace?.role === 'owner'} />
      <Section
        description="Used only to find pull requests that ask for your review."
        id="github-username-heading"
        title="GitHub username"
      >
        <form className="aw-inline-form" onSubmit={(event) => { event.preventDefault(); saveGithub.mutate() }}>
          <TextField
            id="account-github"
            label="GitHub username"
            maxLength={39}
            onChange={(event) => setGithubDraft(event.target.value)}
            value={githubLogin}
          />
          <Button
            busy={saveGithub.isPending}
            disabled={profile.isPending || githubLogin.trim() === (profile.data?.githubLogin ?? '')}
            type="submit"
            variant="primary"
          >
            Save
          </Button>
        </form>
      </Section>
      <Section
        description="Link a single sign-on account to keep your projects, conversations and username when you sign in with it."
        id="sign-in-heading"
        title="Sign-in"
      >
        {status.isPending && <SettingsSkeleton label="Loading sign-in settings" sections={1} />}
        {status.isError && (
          <Banner onRetry={() => void status.refetch()} title="Sign-in settings didn’t load." tone="danger">
            {describeLoadError(status.error)}
          </Banner>
        )}
        <div className="aw-form__row">
          {status.data?.googleEnabled && (
            <Button busy={link.isPending} onClick={() => link.mutate('GoogleOAuth')}>
              Link Google account
            </Button>
          )}
          {status.data?.enabled &&
            (status.data.linked ? (
              <p className="aw-quiet">Single sign-on linked as {status.data.email}.</p>
            ) : (
              <Button busy={link.isPending} onClick={() => link.mutate(undefined)}>
                Link single sign-on account
              </Button>
            ))}
        </div>
        {status.data && !status.data.enabled && <p className="aw-quiet">Single sign-on isn’t set up on this server.</p>}
      </Section>
    </>
  )
}
