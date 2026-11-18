import { useRef, useState, type ChangeEvent, type FormEvent } from 'react'

import { toastError } from '../app/toastError'
import { TimeZoneField } from '../settings/TimeZoneField'
import { deviceTimeZone } from '../settings/timeZones'
import { Avatar, Banner, Button, Section, SelectField, SettingsSkeleton, TextField, describeLoadError, showToast } from '../ui'
import { fileBase64, profileKind, useProfile, useProfileUpdate, type Profile } from './api'
import './profiles.css'

const AVATAR_TYPES = 'image/png,image/jpeg,image/gif,image/webp'
const MAX_AVATAR = 256 * 1024

/**
 * Account › Profile › About you (C2.17): your picture, title, pronouns and
 * time zone, shown to everyone who shares a workspace with you. `account`
 * edits an agent you manage instead.
 */
export function ProfileSection({ account, name }: { account?: string; name: string }) {
  const profile = useProfile(account ?? null)
  return (
    <Section
      description="Shown to everyone who shares a workspace with you, next to your name."
      id="about-you-heading"
      title="About you"
    >
      {profile.isPending ? <SettingsSkeleton label="Loading your profile" sections={1} /> : null}
      {profile.isError ? (
        <Banner onRetry={() => void profile.refetch()} title="Your profile didn’t load." tone="danger">
          {describeLoadError(profile.error)}
        </Banner>
      ) : null}
      {profile.data ? <ProfileForm account={account} key={profile.data.accountId} name={name} profile={profile.data} /> : null}
    </Section>
  )
}

function ProfileForm({ account, name, profile }: { account?: string; name: string; profile: Profile }) {
  const update = useProfileUpdate()
  const file = useRef<HTMLInputElement>(null)
  const [title, setTitle] = useState(profile.title ?? '')
  const [pronouns, setPronouns] = useState(profile.pronouns ?? '')
  const [timeZone, setTimeZone] = useState(profile.timeZone ?? deviceTimeZone())
  const [busy, setBusy] = useState<'save' | 'avatar' | null>(null)
  const [error, setError] = useState<string | null>(null)
  const changed =
    title.trim() !== (profile.title ?? '') || pronouns.trim() !== (profile.pronouns ?? '') || timeZone !== (profile.timeZone ?? '')

  const save = (event: FormEvent) => {
    event.preventDefault()
    setBusy('save')
    update({ account, title: title.trim(), pronouns: pronouns.trim(), timeZone })
      .then(() => showToast({ tone: 'success', title: 'Profile saved' }))
      .catch((reason: unknown) => toastError('Your profile wasn’t saved', reason))
      .finally(() => setBusy(null))
  }
  const pick = (event: ChangeEvent<HTMLInputElement>) => {
    const chosen = event.target.files?.[0]
    event.target.value = ''
    if (!chosen) return
    if (chosen.size > MAX_AVATAR) {
      setError('Pick an image of at most 256 KB.')
      return
    }
    setError(null)
    setBusy('avatar')
    fileBase64(chosen)
      .then((avatarBase64) => update({ account, avatarBase64 }))
      .then(() => showToast({ tone: 'success', title: 'Picture changed' }))
      .catch((reason: unknown) => toastError('Your picture didn’t change', reason))
      .finally(() => setBusy(null))
  }
  const setDmPolicy = (dmPolicy: string) => {
    update({ account, dmPolicy })
      .then(() => showToast({ tone: 'success', title: dmPolicy === 'ask' ? 'New people ask first' : 'Anyone you share a workspace with can message you' }))
      .catch((reason: unknown) => toastError('Your setting wasn’t saved', reason))
  }
  const remove = () => {
    setBusy('avatar')
    update({ account, removeAvatar: true })
      .then(() => showToast({ tone: 'success', title: 'Picture removed' }))
      .catch((reason: unknown) => toastError('Your picture wasn’t removed', reason))
      .finally(() => setBusy(null))
  }

  return (
    <form className="aw-profile-form" onSubmit={save}>
      <div className="aw-profile-form__picture">
        <Avatar kind={profileKind(profile.kind)} name={name} seed={profile.accountId} size={64} src={profile.avatarUrl} />
        <div className="aw-form__row">
          <Button busy={busy === 'avatar'} onClick={() => file.current?.click()}>
            {profile.avatarUrl ? 'Change picture' : 'Add a picture'}
          </Button>
          {profile.avatarUrl ? (
            <Button disabled={busy === 'avatar'} onClick={remove} variant="ghost">
              Remove
            </Button>
          ) : null}
          <input accept={AVATAR_TYPES} aria-label="Picture file" hidden onChange={pick} ref={file} type="file" />
        </div>
        {error ? <p className="aw-field__note" role="alert">{error}</p> : <p className="aw-quiet">PNG, JPEG, GIF or WebP, up to 256 KB.</p>}
      </div>
      <TextField id="profile-title" label="Title" maxLength={80} onChange={(event) => setTitle(event.target.value)} placeholder="What you do" value={title} />
      <TextField id="profile-pronouns" label="Pronouns" maxLength={40} onChange={(event) => setPronouns(event.target.value)} placeholder="Such as she/her" value={pronouns} />
      <TimeZoneField hint="Others see your local time." id="profile-time-zone" onChange={setTimeZone} value={timeZone} />
      {profile.dmPolicy ? (
        <SelectField
          hint="With “Ask me first”, someone’s first message is a request you accept or decline."
          id="profile-dm-policy"
          label="Who can message you"
          onChange={(event) => setDmPolicy(event.target.value)}
          value={profile.dmPolicy}
        >
          <option value="anyone">Anyone you share a workspace with</option>
          <option value="ask">Ask me first</option>
        </SelectField>
      ) : null}
      <div className="aw-form__row">
        <Button busy={busy === 'save'} disabled={!changed} type="submit" variant="primary">
          Save
        </Button>
      </div>
    </form>
  )
}
