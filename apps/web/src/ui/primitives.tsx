import {
  forwardRef,
  useEffect,
  useId,
  useRef,
  type ButtonHTMLAttributes,
  type InputHTMLAttributes,
  type ReactNode,
  type SelectHTMLAttributes,
  type TextareaHTMLAttributes,
} from 'react'
import { createPortal } from 'react-dom'

type ButtonVariant = 'primary' | 'secondary' | 'danger' | 'quiet'

export const Button = forwardRef<
  HTMLButtonElement,
  ButtonHTMLAttributes<HTMLButtonElement> & {
    variant?: ButtonVariant
    busy?: boolean
  }
>(function Button(
  { children, className = '', variant = 'secondary', busy = false, disabled, ...props },
  ref,
) {
  return (
    <button
      aria-busy={busy || undefined}
      className={`ui-button ui-button--${variant} ${className}`.trim()}
      disabled={disabled || busy}
      ref={ref}
      {...props}
    >
      {children}
    </button>
  )
})

export const IconButton = forwardRef<
  HTMLButtonElement,
  ButtonHTMLAttributes<HTMLButtonElement> & { label: string }
>(function IconButton({ children, className = '', label, ...props }, ref) {
  return (
    <button
      aria-label={label}
      className={`ui-icon-button ${className}`.trim()}
      ref={ref}
      {...props}
    >
      {children}
    </button>
  )
})

type FieldProps = {
  label: ReactNode
  description?: ReactNode
  error?: ReactNode
}

function FieldFrame({
  children,
  description,
  descriptionId,
  error,
  errorId,
  htmlFor,
  label,
}: FieldProps & {
  children: ReactNode
  descriptionId: string
  errorId: string
  htmlFor: string
}) {
  return (
    <div className="ui-field">
      <label htmlFor={htmlFor}>{label}</label>
      {children}
      {description ? <small id={descriptionId}>{description}</small> : null}
      {error ? <p id={errorId} role="alert">{error}</p> : null}
    </div>
  )
}

export const TextInput = forwardRef<
  HTMLInputElement,
  InputHTMLAttributes<HTMLInputElement> & FieldProps
>(function TextInput({ description, error, id, label, ...props }, ref) {
  const generatedId = useId()
  const inputId = id ?? generatedId
  const descriptionId = `${inputId}-description`
  const errorId = `${inputId}-error`
  return (
    <FieldFrame description={description} descriptionId={descriptionId} error={error} errorId={errorId} htmlFor={inputId} label={label}>
      <input aria-describedby={[description && descriptionId, error && errorId].filter(Boolean).join(' ') || undefined} aria-invalid={Boolean(error) || undefined} id={inputId} ref={ref} {...props} />
    </FieldFrame>
  )
})

export const TextArea = forwardRef<
  HTMLTextAreaElement,
  TextareaHTMLAttributes<HTMLTextAreaElement> & FieldProps
>(function TextArea({ description, error, id, label, ...props }, ref) {
  const generatedId = useId()
  const inputId = id ?? generatedId
  const descriptionId = `${inputId}-description`
  const errorId = `${inputId}-error`
  return (
    <FieldFrame description={description} descriptionId={descriptionId} error={error} errorId={errorId} htmlFor={inputId} label={label}>
      <textarea aria-describedby={[description && descriptionId, error && errorId].filter(Boolean).join(' ') || undefined} aria-invalid={Boolean(error) || undefined} id={inputId} ref={ref} {...props} />
    </FieldFrame>
  )
})

export const Select = forwardRef<
  HTMLSelectElement,
  SelectHTMLAttributes<HTMLSelectElement> & FieldProps
>(function Select({ children, description, error, id, label, ...props }, ref) {
  const generatedId = useId()
  const inputId = id ?? generatedId
  const descriptionId = `${inputId}-description`
  const errorId = `${inputId}-error`
  return (
    <FieldFrame description={description} descriptionId={descriptionId} error={error} errorId={errorId} htmlFor={inputId} label={label}>
      <select aria-describedby={[description && descriptionId, error && errorId].filter(Boolean).join(' ') || undefined} aria-invalid={Boolean(error) || undefined} id={inputId} ref={ref} {...props}>{children}</select>
    </FieldFrame>
  )
})

export function SearchField({ label, ...props }: InputHTMLAttributes<HTMLInputElement> & { label: string }) {
  return (
    <label className="ui-search-field">
      <span aria-hidden="true">⌕</span>
      <span className="visually-hidden">{label}</span>
      <input type="search" {...props} />
    </label>
  )
}

export function Checkbox({ label, ...props }: InputHTMLAttributes<HTMLInputElement> & { label: ReactNode }) {
  return <label className="ui-check"><input type="checkbox" {...props} /><span>{label}</span></label>
}

export function Switch({ label, ...props }: InputHTMLAttributes<HTMLInputElement> & { label: ReactNode }) {
  return <label className="ui-switch"><input role="switch" type="checkbox" {...props} /><span aria-hidden="true" className="ui-switch-track" /><span>{label}</span></label>
}

const focusableSelector = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'

export function Dialog({ children, label, onClose, open }: { children: ReactNode; label: string; onClose: () => void; open: boolean }) {
  const panelRef = useRef<HTMLDivElement>(null)
  const restoreRef = useRef<HTMLElement | null>(null)
  useEffect(() => {
    if (!open) return
    restoreRef.current = document.activeElement as HTMLElement | null
    const panel = panelRef.current
    const applicationRoot = document.querySelector<HTMLElement>('#root')
    const applicationWasInert = applicationRoot?.hasAttribute('inert') ?? false
    applicationRoot?.setAttribute('inert', '')
    panel?.querySelector<HTMLElement>(focusableSelector)?.focus()
    const keydown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        onClose()
        return
      }
      if (event.key !== 'Tab' || !panel) return
      const controls = Array.from(panel.querySelectorAll<HTMLElement>(focusableSelector))
      if (!controls.length) return
      const first = controls[0]
      const last = controls[controls.length - 1]
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus() }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus() }
    }
    document.addEventListener('keydown', keydown)
    return () => {
      document.removeEventListener('keydown', keydown)
      if (!applicationWasInert) applicationRoot?.removeAttribute('inert')
      window.requestAnimationFrame(() => restoreRef.current?.focus())
    }
  }, [onClose, open])
  if (!open) return null
  return createPortal(
    <div className="ui-overlay" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <div aria-label={label} aria-modal="true" className="ui-dialog" ref={panelRef} role="dialog">{children}</div>
    </div>,
    document.body,
  )
}

export function Drawer({ children, label, onClose, open, side = 'right' }: { children: ReactNode; label: string; onClose: () => void; open: boolean; side?: 'left' | 'right' }) {
  return (
    <Dialog label={label} onClose={onClose} open={open}>
      <div className="ui-drawer" data-side={side}>{children}</div>
    </Dialog>
  )
}

export function Popover({ children, label, open }: { children: ReactNode; label: string; open: boolean }) {
  return open ? <div aria-label={label} className="ui-popover" role="dialog">{children}</div> : null
}

export function Tooltip({ children, content }: { children: ReactNode; content: ReactNode }) {
  const id = useId()
  return <span aria-describedby={id} className="ui-tooltip-trigger">{children}<span className="ui-tooltip" id={id} role="tooltip">{content}</span></span>
}

export function Menu({ children, label }: { children: ReactNode; label: string }) {
  return <div aria-label={label} className="ui-menu" role="menu">{children}</div>
}

export function Tabs({ children, label }: { children: ReactNode; label: string }) {
  return <div aria-label={label} className="ui-tabs" role="tablist">{children}</div>
}

export function SegmentedControl({ children, label }: { children: ReactNode; label: string }) {
  return <div aria-label={label} className="ui-segmented" role="group">{children}</div>
}

export function Avatar({ kind = 'human', label, text }: { kind?: string; label: string; text: string }) {
  return <span aria-label={label} className="ui-avatar" data-identity={kind} role="img">{text.slice(0, 2).toLocaleUpperCase()}</span>
}

export function IdentityBadge({ kind, name }: { kind: string; name: string }) {
  return <span className="ui-identity"><Avatar kind={kind} label={`${name}, ${kind}`} text={name} /><span><strong>{name}</strong><small>{kind.replaceAll('_', ' ')}</small></span></span>
}

export function StatusBadge({ children, status }: { children: ReactNode; status: string }) {
  return <span className="ui-status" data-status={status}><span aria-hidden="true" className="ui-status-mark" />{children}</span>
}

export function Skeleton({ label = 'Loading', lines = 3 }: { label?: string; lines?: number }) {
  return <div aria-busy="true" aria-label={label} className="ui-skeleton">{Array.from({ length: lines }, (_, index) => <span key={index} />)}</div>
}

export function Spinner({ label = 'Loading' }: { label?: string }) {
  return <span aria-label={label} className="ui-spinner" role="status" />
}

export function Progress({ label, value }: { label: string; value?: number }) {
  return <progress aria-label={label} className="ui-progress" max={100} value={value} />
}

export function EmptyState({ action, description, icon = '◇', title }: { action?: ReactNode; description: ReactNode; icon?: ReactNode; title: string }) {
  return <div className="ui-empty"><span aria-hidden="true" className="ui-empty-icon">{icon}</span><h2>{title}</h2><p>{description}</p>{action}</div>
}

export function ErrorState({ action, description, title = 'Something went wrong' }: { action?: ReactNode; description: ReactNode; title?: string }) {
  return <div className="ui-error" role="alert"><h2>{title}</h2><p>{description}</p>{action}</div>
}

export function Announcement({ children, urgent = false }: { children: ReactNode; urgent?: boolean }) {
  return <p aria-live={urgent ? 'assertive' : 'polite'} className="visually-hidden" role={urgent ? 'alert' : 'status'}>{children}</p>
}

export function Toast({ children, urgent = false }: { children: ReactNode; urgent?: boolean }) {
  return <div aria-live={urgent ? 'assertive' : 'polite'} className="ui-toast" role={urgent ? 'alert' : 'status'}>{children}</div>
}
