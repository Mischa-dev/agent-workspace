import type { ReactNode } from 'react'

/** One part of a task's page (subtasks, criteria, links…): a quiet heading with a count, actions on the right. */
export function TaskSection({
  actions,
  children,
  count,
  id,
  title,
}: {
  actions?: ReactNode
  children: ReactNode
  count?: ReactNode
  id: string
  title: string
}) {
  return (
    <section aria-labelledby={id} className="aw-tsection">
      <header className="aw-tsection__head">
        <h2 className="aw-tsection__title" id={id}>
          {title}
          {count !== undefined && count !== null ? <span className="aw-tsection__count">{count}</span> : null}
        </h2>
        {actions ? <div className="aw-tsection__actions">{actions}</div> : null}
      </header>
      {children}
    </section>
  )
}
