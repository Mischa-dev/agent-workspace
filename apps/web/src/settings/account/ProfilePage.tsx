import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { requestJson } from '../../api/client'
import { queryKeys } from '../../api/queryKeys'
import { isApiError } from '../../api/errors'
import { startWorkOs, workOsStatus } from '../../auth/workos'
import { Avatar, Banner, Button, PageHeader, Section, TextField } from '../../ui'
import type { SettingsPageProps } from '../types'

/** Account › Profile: your name, your GitHub username and how you sign in. */
export default function ProfilePage({ session }: SettingsPageProps) {
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
      await client.invalidateQueries({ queryKey: ['account-profile'] })
    },
    onError: (reason) => setError(isApiError(reason) ? reason.message : 'Could not save your GitHub username.'),
  })
  const save = useMutation({
    mutationFn: () => requestJson('/api/v1/account/profile', { method: 'PATCH', json: { displayName: name.trim() } }),
    onSuccess: async () => {
      setError(null)
      await client.invalidateQueries({ queryKey: queryKeys.session })
    },
    onError: (reason) => setError(isApiError(reason) ? reason.message : 'Could not save your profile.'),
  })
  const link = useMutation({
    mutationFn: (provider?: 'GoogleOAuth') => startWorkOs({ link: true, provider }),
    onError: () => setError('Could not open secure sign-in. Try again.'),
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
        Your identity stays with you across workspaces and Projects.
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
        {save.isSuccess && <p className="aw-quiet" role="status">Profile saved.</p>}
      </Section>
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
          <Button busy={saveGithub.isPending} disabled={profile.isPending} type="submit">
            Save
          </Button>
        </form>
        {saveGithub.isSuccess && <p className="aw-quiet" role="status">GitHub username saved.</p>}
      </Section>
      <Section
        description="Link a single sign-on account to keep your Projects, conversations and username when you sign in with it."
        id="sign-in-heading"
        title="Sign-in"
      >
        {status.isPending && <p className="aw-quiet">Loading sign-in settings…</p>}
        {status.isError && <Banner tone="danger">Could not load sign-in settings.</Banner>}
        <div className="aw-form__row">
          {status.data?.googleEnabled && (
            <Button busy={link.isPending} onClick={() => link.mutate('GoogleOAuth')}>
              Link Google account
            </Button>
          )}
          {status.data?.enabled &&
            (status.data.linked ? (
              <p className="aw-quiet">WorkOS connected as {status.data.email}.</p>
            ) : (
              <Button busy={link.isPending} onClick={() => link.mutate(undefined)}>
                Link WorkOS account
              </Button>
            ))}
        </div>
        {status.data && !status.data.enabled && <p className="aw-quiet">Single sign-on isn’t set up on this server.</p>}
      </Section>
    </>
  )
}
