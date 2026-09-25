import type { ReactNode } from 'react'

import { Icon, type IconName } from './icons'

export type BannerTone = 'info' | 'warning' | 'danger' | 'success' | 'agent' | 'offline'

const BANNER_ICONS: Record<BannerTone, IconName> = {
  info: 'info',
  warning: 'triangle-alert',
  danger: 'circle-alert',
  success: 'circle-check',
  agent: 'bot',
  offline: 'wifi-off',
}

/** A full-width note about the state of the pane, with an optional fix. */
export function Banner({
  action,
  children,
  className,
  icon,
  role,
  title,
  tone = 'info',
}: {
  action?: ReactNode
  children?: ReactNode
  className?: string
  icon?: IconName
  /** Overrides the live-region role; danger banners are alerts, the rest are status by default. */
  role?: 'alert' | 'status'
  title?: ReactNode
  tone?: BannerTone
}) {
  return (
    <div
      className={['aw-banner', `aw-banner--${tone}`, className].filter(Boolean).join(' ')}
      role={role ?? (tone === 'danger' ? 'alert' : 'status')}
    >
      <Icon className="aw-banner__icon" name={icon ?? BANNER_ICONS[tone]} />
      <p className="aw-banner__text">
        {title ? <strong>{title} </strong> : null}
        {children}
      </p>
      {action}
    </div>
  )
}

export function EmptyState({
  action,
  children,
  icon = 'info',
  title,
}: {
  action?: ReactNode
  children?: ReactNode
  icon?: IconName
  title: string
}) {
  return (
    <section className="aw-empty">
      <span className="aw-empty__icon">
        <Icon name={icon} size={24} />
      </span>
      <h2 className="aw-empty__title">{title}</h2>
      {children ? <p className="aw-empty__text">{children}</p> : null}
      {action}
    </section>
  )
}

/** The first thing in a conversation: where you are and what to do first. */
export function ConversationIntro({
  actions,
  children,
  icon = 'hash',
  mark,
  title,
}: {
  actions?: ReactNode
  children?: ReactNode
  icon?: IconName
  /** Replaces the icon tile, for example with an avatar. */
  mark?: ReactNode
  title: ReactNode
}) {
  return (
    <section className="aw-intro">
      {mark ?? (
        <span className="aw-intro__tile">
          <Icon name={icon} size={28} />
        </span>
      )}
      <h2 className="aw-intro__title">{title}</h2>
      {children ? <p className="aw-intro__text">{children}</p> : null}
      {actions ? <div className="aw-intro__actions">{actions}</div> : null}
    </section>
  )
}

const NAME_WIDTHS = [28, 20, 34]
const LINE_WIDTHS = [74, 56, 86]

export function MessageSkeleton({
  decorative = false,
  label = 'Loading messages',
  rows = 3,
}: {
  /** Hide it from assistive technology when something else already announces the loading. */
  decorative?: boolean
  label?: string
  rows?: number
}) {
  return (
    <div
      aria-busy="true"
      aria-hidden={decorative || undefined}
      aria-label={decorative ? undefined : label}
      className="aw-skel"
      role={decorative ? undefined : 'status'}
    >
      {Array.from({ length: rows }, (_, index) => (
        <div className="aw-skel__row" key={index}>
          <span className="aw-skel__avatar" />
          <div className="aw-skel__lines">
            <span className="aw-skel__line is-name" style={{ width: `${NAME_WIDTHS[index % 3]}%` }} />
            <span className="aw-skel__line" style={{ width: `${LINE_WIDTHS[index % 3]}%` }} />
            {index % 2 === 0 ? <span className="aw-skel__line" style={{ width: '42%' }} /> : null}
          </div>
        </div>
      ))}
    </div>
  )
}

export function ErrorState({
  action,
  children,
  title,
}: {
  action?: ReactNode
  children?: ReactNode
  title: string
}) {
  return (
    <section className="aw-empty aw-empty--error" role="alert">
      <span className="aw-empty__icon">
        <Icon name="circle-alert" size={24} />
      </span>
      <h2 className="aw-empty__title">{title}</h2>
      {children ? <p className="aw-empty__text">{children}</p> : null}
      {action}
    </section>
  )
}
