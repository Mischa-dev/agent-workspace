import type { ReactNode } from 'react'

/**
 * Loading placeholders shaped like the page that's coming, so the layout
 * doesn't jump when it arrives. Chat timelines keep MessageSkeleton.
 */

const WIDTHS = [62, 44, 71, 53, 38, 66]

function Bar({ width, className = '' }: { width: number | string; className?: string }) {
  return <span className={`aw-skel__bar ${className}`.trim()} style={{ width: typeof width === 'number' ? `${width}%` : width }} />
}

function Frame({
  children,
  className,
  decorative,
  label,
}: {
  children: ReactNode
  className: string
  decorative: boolean
  label: string
}) {
  return (
    <div
      aria-busy="true"
      aria-hidden={decorative || undefined}
      aria-label={decorative ? undefined : label}
      className={`aw-skel aw-skel--shaped ${className}`}
      role={decorative ? undefined : 'status'}
    >
      {children}
    </div>
  )
}

type SkeletonProps = {
  /** Hide it from assistive technology when something else already announces the loading. */
  decorative?: boolean
  label?: string
}

/** Rows of a list: an icon or avatar, a title and a quieter line. */
export function ListSkeleton({ decorative = false, label = 'Loading', rows = 5, avatars = false }: SkeletonProps & { rows?: number; avatars?: boolean }) {
  return (
    <Frame className="aw-skel--list" decorative={decorative} label={label}>
      {Array.from({ length: rows }, (_, index) => (
        <div className="aw-skel__listrow" key={index}>
          <span className={avatars ? 'aw-skel__avatar' : 'aw-skel__icon'} />
          <div className="aw-skel__lines">
            <Bar className="is-strong" width={WIDTHS[index % WIDTHS.length]} />
            <Bar width={WIDTHS[(index + 3) % WIDTHS.length] - 12} />
          </div>
          <Bar className="aw-skel__trailing" width="56px" />
        </div>
      ))}
    </Frame>
  )
}

/** A settings form: sections of labelled fields. */
export function SettingsSkeleton({ decorative = false, label = 'Loading settings', sections = 2 }: SkeletonProps & { sections?: number }) {
  return (
    <Frame className="aw-skel--settings" decorative={decorative} label={label}>
      {Array.from({ length: sections }, (_, section) => (
        <div className="aw-skel__section" key={section}>
          <Bar className="is-heading" width={section % 2 ? '28%' : '36%'} />
          <Bar width="64%" />
          {Array.from({ length: 2 }, (_, field) => (
            <div className="aw-skel__field" key={field}>
              <Bar className="is-label" width="22%" />
              <span className="aw-skel__control" />
            </div>
          ))}
        </div>
      ))}
    </Frame>
  )
}

/** A page with a header, a short summary and a few cards. */
export function PageSkeleton({ decorative = false, label = 'Loading page', cards = 3 }: SkeletonProps & { cards?: number }) {
  return (
    <Frame className="aw-skel--page" decorative={decorative} label={label}>
      <div className="aw-skel__pagehead">
        <Bar className="is-title" width="34%" />
        <Bar width="52%" />
      </div>
      <div className="aw-skel__cards">
        {Array.from({ length: cards }, (_, index) => (
          <div className="aw-skel__card" key={index}>
            <Bar className="is-strong" width={WIDTHS[index % WIDTHS.length]} />
            <Bar width="80%" />
            <Bar width="58%" />
          </div>
        ))}
      </div>
    </Frame>
  )
}

/** A document: a title and paragraphs. */
export function DocumentSkeleton({ decorative = false, label = 'Loading', paragraphs = 3 }: SkeletonProps & { paragraphs?: number }) {
  return (
    <Frame className="aw-skel--document" decorative={decorative} label={label}>
      <Bar className="is-title" width="46%" />
      {Array.from({ length: paragraphs }, (_, index) => (
        <div className="aw-skel__paragraph" key={index}>
          <Bar width="96%" />
          <Bar width="91%" />
          <Bar width={WIDTHS[index % WIDTHS.length]} />
        </div>
      ))}
    </Frame>
  )
}
