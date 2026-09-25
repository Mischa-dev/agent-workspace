import type { ReactNode } from 'react'

import { Icon, type IconName } from './icons'

/** A settings-style page: a title, a sentence about it, and actions on the right. */
export function PageHeader({
  actions,
  children,
  icon,
  title,
}: {
  actions?: ReactNode
  children?: ReactNode
  icon?: IconName
  title: string
}) {
  return (
    <header className="aw-pagehead">
      <div className="aw-pagehead__text">
        <h1 className="aw-pagehead__title" id="workspace-heading">
          {icon ? (
            <span className="aw-pagehead__icon">
              <Icon name={icon} size={20} />
            </span>
          ) : null}
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
  title,
}: {
  actions?: ReactNode
  children: ReactNode
  description?: ReactNode
  title: string
}) {
  return (
    <section className="aw-pagesection">
      <div className="aw-pagesection__head">
        <div>
          <h2 className="aw-pagesection__title">{title}</h2>
          {description ? <p className="aw-pagesection__desc">{description}</p> : null}
        </div>
        {actions}
      </div>
      {children}
    </section>
  )
}
