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
  const [copied, setCopied] = useState(false)
  const timer = useRef<number | undefined>(undefined)
  const heading = useRef<HTMLHeadingElement>(null)
  useEffect(() => {
    heading.current?.focus()
    return () => window.clearTimeout(timer.current)
  }, [])
  const copy = () => {
    void navigator.clipboard
      ?.writeText(value)
      .then(() => {
        setCopied(true)
        window.clearTimeout(timer.current)
        timer.current = window.setTimeout(() => setCopied(false), 2000)
      })
      .catch(() => undefined)
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
        <Button icon={copied ? 'check' : 'copy'} onClick={copy} variant="primary">
          {copied ? 'Copied' : copyLabel}
        </Button>
        {extra}
        <Button onClick={onDone}>Done</Button>
      </div>
    </aside>
  )
}
