import type { ReactNode } from 'react'

/** The product mark: a person (round) and an agent (square), side by side. */
export function BrandMark({ size = 40 }: { size?: number }) {
  return (
    <span aria-hidden="true" className="aw-logo" style={{ width: size, height: size }}>
      <span className="aw-logo__person" />
      <span className="aw-logo__agent" />
    </span>
  )
}

/** Sign-in and invitation pages: the brand on one side, the form on the other. */
export function AuthLayout({
  children,
  footer,
  intro,
  kicker,
  title,
  titleId,
}: {
  children: ReactNode
  footer?: ReactNode
  intro?: ReactNode
  kicker?: string
  title: string
  titleId: string
}) {
  return (
    <main className="aw-auth">
      <section aria-hidden="true" className="aw-auth__brand">
        <div className="aw-auth__brand-top">
          <BrandMark />
          <span className="aw-auth__wordmark">Agent Workspace</span>
        </div>
        <div className="aw-auth__pattern">
          {'PAPPAPAAPPAPPPAA'.split('').map((kind, index) => (
            <span className={kind === 'P' ? 'is-person' : 'is-agent'} key={index} />
          ))}
        </div>
        <p className="aw-auth__tagline">Chat for people and their agents.</p>
      </section>
      <section aria-labelledby={titleId} className="aw-auth__panel">
        <div className="aw-auth__card">
          <div className="aw-auth__mobile-brand">
            <BrandMark size={32} />
            <span>Agent Workspace</span>
          </div>
          {kicker ? <p className="aw-auth__kicker">{kicker}</p> : null}
          <h1 className="aw-auth__title" id={titleId}>
            {title}
          </h1>
          {intro ? <p className="aw-auth__intro">{intro}</p> : null}
          {children}
          {footer ? <div className="aw-auth__footer">{footer}</div> : null}
        </div>
      </section>
    </main>
  )
}
