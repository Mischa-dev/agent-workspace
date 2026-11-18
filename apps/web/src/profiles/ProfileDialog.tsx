import { Link } from 'react-router-dom'

import { projectLocationHref } from '../projects/locations'
import { accountSettingsHref } from '../settings/paths'
import { StatusText } from '../status/StatusText'
import { ActorTag, Avatar, Banner, ButtonLink, DetailList, Dialog, ListSkeleton, describeLoadError } from '../ui'
import { localTime, profileKind, profileStatus, useProfile } from './api'
import './profiles.css'

/**
 * Someone's profile (C2.17), opened from their name: who they are, their
 * pronouns and local time, their status, and the spaces you share with them
 * in this workspace.
 */
export default function ProfileDialog({ accountId, name, onClose }: { accountId: string; name: string; onClose: () => void }) {
  const profile = useProfile(accountId)
  const shown = profile.data
  const kind = profileKind(shown?.kind ?? 'person')
  const time = localTime(shown?.timeZone)
  const status = shown ? profileStatus(shown) : null
  const details = shown
    ? [
        ...(shown.pronouns ? [{ label: 'Pronouns', value: shown.pronouns }] : []),
        ...(time ? [{ label: 'Local time', value: `${time} (${shown.timeZone})` }] : []),
        ...(shown.handle ? [{ label: 'Handle', value: `@${shown.handle}` }] : []),
      ]
    : []
  return (
    <Dialog
      footer={
        shown?.canEdit && kind === 'person' ? (
          <ButtonLink icon="pencil" onClick={onClose} to={accountSettingsHref('profile')}>
            Edit your profile
          </ButtonLink>
        ) : null
      }
      onClose={onClose}
      open
      title={shown?.name || name}
      width={420}
    >
      {profile.isPending ? <ListSkeleton label="Loading the profile" rows={3} /> : null}
      {profile.isError ? (
        <Banner onRetry={() => void profile.refetch()} title="The profile didn’t load." tone="danger">
          {describeLoadError(profile.error)}
        </Banner>
      ) : null}
      {shown ? (
        <div className="aw-person">
          <div className="aw-person__head">
            <Avatar kind={kind} name={shown.name || name} seed={shown.accountId} size={64} src={shown.avatarUrl} />
            <div className="aw-person__who">
              <span className="aw-person__name">
                {shown.name || name} <ActorTag kind={kind} />
              </span>
              {shown.title ? <span className="aw-person__title">{shown.title}</span> : null}
              <StatusText status={status} />
            </div>
          </div>
          {details.length ? <DetailList items={details} /> : null}
          {shown.sharedSpaces.length ? (
            <section aria-label="Spaces you share" className="aw-person__spaces">
              <h3>Spaces you share</h3>
              <ul>
                {shown.sharedSpaces.map((space) => (
                  <li key={space.id}>
                    <Link onClick={onClose} to={projectLocationHref(space.id, { kind: 'project_overview' })}>
                      {space.name}
                    </Link>
                  </li>
                ))}
              </ul>
            </section>
          ) : null}
        </div>
      ) : null}
    </Dialog>
  )
}
