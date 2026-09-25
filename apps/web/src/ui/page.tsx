import type { ReactNode } from 'react'
import { Link } from 'react-router-dom'

import { Icon, type IconName } from './icons'

/** A settings-style page: a title, a sentence about it, and actions on the right. */
export function PageHeader({
  actions,
  children,
  icon,
  mark,
  title,
}: {
  actions?: ReactNode
  children?: ReactNode
  icon?: IconName
  /** Replaces the icon tile, for example with a Project's tile. */
  mark?: ReactNode
  title: string
}) {
  return (
    <header className="aw-pagehead">
      <div className="aw-pagehead__text">
        <h1 className="aw-pagehead__title" id="workspace-heading">
          {mark ??
            (icon ? (
              <span className="aw-pagehead__icon">
                <Icon name={icon} size={20} />
              </span>
            ) : null)}
          {title}
        </h1>
        {children ? <p className="aw-pagehead__lede">{children}</p> : null}
      </div>
      {actions ? <div className="aw-pagehead__actions">{actions}</div> : null}
    </header>
  )
}

/** A titled group of settings or content on a page. */
export function Section({
  actions,
  children,
  description,
  id,
  title,
}: {
  actions?: ReactNode
  children: ReactNode
  description?: ReactNode
  id?: string
  title: string
}) {
  return (
    <section aria-labelledby={id} className="aw-pagesection">
      <div className="aw-pagesection__head">
        <div>
          <h2 className="aw-pagesection__title" id={id}>
            {title}
          </h2>
          {description ? <p className="aw-pagesection__desc">{description}</p> : null}
        </div>
        {actions}
      </div>
      {children}
    </section>
  )
}

/** Links between the parts of one page, such as the sections of Project settings. */
export function PageTabs({
  current,
  label,
  tabs,
}: {
  current: string
  label: string
  tabs: { id: string; label: string; to: string; icon?: IconName }[]
}) {
  return (
    <nav aria-label={label} className="aw-tabs">
      {tabs.map((tab) => (
        <Link
          aria-current={tab.id === current ? 'page' : undefined}
          className="aw-tabs__tab"
          key={tab.id}
          replace
          to={tab.to}
        >
          {tab.icon ? <Icon name={tab.icon} size={16} /> : null}
          {tab.label}
        </Link>
      ))}
    </nav>
  )
}

/** Label and value pairs, such as a Project's key and your role in it. */
export function DetailList({ items }: { items: { label: string; value: ReactNode }[] }) {
  return (
    <dl className="aw-dl">
      {items.map((item) => (
        <div className="aw-dl__row" key={item.label}>
          <dt>{item.label}</dt>
          <dd>{item.value}</dd>
        </div>
      ))}
    </dl>
  )
}

/** A bordered group of rows: members, repositories, Projects. */
export function RowList({ children, label }: { children: ReactNode; label?: string }) {
  return (
    <ul aria-label={label} className="aw-rows">
      {children}
    </ul>
  )
}

export function Row({
  actions,
  children,
  className,
  heading = false,
  leading,
  meta,
  title,
}: {
  actions?: ReactNode
  children?: ReactNode
  className?: string
  /** Render the title as a heading, for rows that head their own details. */
  heading?: boolean
  leading?: ReactNode
  meta?: ReactNode
  title: ReactNode
}) {
  const Title = heading ? 'h3' : 'div'
  return (
    <li className={className ? `aw-row ${className}` : 'aw-row'}>
      {leading ? <span className="aw-row__lead">{leading}</span> : null}
      <div className="aw-row__main">
        <Title className="aw-row__title">{title}</Title>
        {meta ? <div className="aw-row__meta">{meta}</div> : null}
        {children}
      </div>
      {actions ? <div className="aw-row__actions">{actions}</div> : null}
    </li>
  )
}
