import { useEffect, useRef, useState, type ReactNode } from 'react'

import { Button, Icon, type IconName } from '../ui'

/** A secret shown once, such as a new token or an invitation link: copy it, then it's gone. */
export function OneTimeValue({
  children,
  copyLabel = 'Copy',
  extra,
  icon = 'key',
  onDone,
  title,
  value,
}: {
  children: ReactNode
  copyLabel?: string
  extra?: ReactNode
  icon?: IconName
  onDone: () => void
  title: string
  value: string
}) {
  const [copied, setCopied] = useState<'yes' | 'failed' | null>(null)
  const timer = useRef<number | undefined>(undefined)
  const heading = useRef<HTMLHeadingElement>(null)
  useEffect(() => {
    heading.current?.focus()
    return () => window.clearTimeout(timer.current)
  }, [])
  const copy = () => {
    // Without a clipboard (an insecure page, a refused permission) say so, so nobody closes it thinking it's copied.
    if (!navigator.clipboard) {
      setCopied('failed')
      return
    }
    navigator.clipboard.writeText(value).then(
      () => {
        setCopied('yes')
        window.clearTimeout(timer.current)
        timer.current = window.setTimeout(() => setCopied(null), 2000)
      },
      () => setCopied('failed'),
    )
  }
  return (
    <aside aria-label={title} className="aw-card aw-token">
      <div className="aw-token__head">
        <Icon name={icon} size={18} />
        <h3 ref={heading} tabIndex={-1}>
          {title}
        </h3>
      </div>
      <p>{children}</p>
      <code className="aw-token__value">{value}</code>
      <div className="aw-card__actions aw-onetime__actions">
        <Button icon={copied === 'yes' ? 'check' : 'copy'} onClick={copy} variant="primary">
          {copied === 'yes' ? 'Copied' : copyLabel}
        </Button>
        {extra}
        <Button onClick={onDone}>Done</Button>
      </div>
      {copied === 'failed' ? (
        <p className="aw-quiet" role="status">
          Couldn’t copy. Select the text above and copy it yourself.
        </p>
      ) : null}
    </aside>
  )
}
