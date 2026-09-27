import { useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { requestJson } from '../api/client'
import { isApiError } from '../api/errors'
import { AppShell } from '../shell/AppShell'
import { Banner, Button, PageHeader, Section, TextField } from '../ui'
import { useAuthenticatedSession } from './session'
import { startWorkOs, workOsStatus } from './workos'

export function AccountSettingsPage() {
  const session = useAuthenticatedSession()
  const client = useQueryClient()
  const [name, setName] = useState(session.user.displayName)
  const [error, setError] = useState<string | null>(null)
  const status = useQuery({ queryKey: ['workos-status'], queryFn: workOsStatus })
  const save = useMutation({
    mutationFn: () => requestJson('/api/v1/account/profile', { method: 'PATCH', json: { displayName: name } }),
    onSuccess: async () => { setError(null); await client.invalidateQueries() },
    onError: (reason) => setError(isApiError(reason) ? reason.message : 'Could not save your profile.'),
  })
  const link = useMutation({
    mutationFn: (provider?: 'GoogleOAuth') => startWorkOs({ link: true, provider }),
    onError: () => setError('Could not open secure sign-in. Try again.'),
  })
  const submit = (event: FormEvent) => { event.preventDefault(); save.mutate() }
  return (
    <AppShell mainClassName="aw-main--page" user={session.user}>
      <div className="aw-page">
        <PageHeader title="Account">Your identity stays with you across projects.</PageHeader>
        {error && <Banner tone="danger">{error}</Banner>}
        <Section title="Profile">
          <form className="aw-inline-form" onSubmit={submit}>
            <TextField id="account-username" label="Username" readOnly value={session.user.loginName} />
            <TextField id="account-name" label="Display name" maxLength={120} required value={name} onChange={(event) => setName(event.target.value)} />
            <Button busy={save.isPending} disabled={!name.trim()} type="submit" variant="primary">Save profile</Button>
          </form>
          {save.isSuccess && <p role="status">Profile saved.</p>}
        </Section>
        <Section title="Sign-in" description="Link WorkOS to keep your existing projects, conversations, and username.">
          {status.isPending && <p>Loading sign-in settings…</p>}
          {status.isError && <Banner tone="danger">Could not load sign-in settings.</Banner>}
          {status.data?.googleEnabled && !status.data.linked && <Button busy={link.isPending} onClick={() => link.mutate('GoogleOAuth')} variant="primary">Link Google account</Button>}
          {status.data?.enabled && (status.data.linked
            ? <p>WorkOS connected as {status.data.email}.</p>
            : <Button busy={link.isPending} onClick={() => link.mutate(undefined)} variant="primary">Link WorkOS account</Button>)}
          {status.data && !status.data.enabled && <p>WorkOS sign-in has not been configured on this host.</p>}
        </Section>
      </div>
    </AppShell>
  )
}
