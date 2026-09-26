import {
  forwardRef,
  useId,
  useRef,
  type ButtonHTMLAttributes,
  type InputHTMLAttributes,
  type KeyboardEvent,
  type ReactNode,
  type SelectHTMLAttributes,
  type TextareaHTMLAttributes,
} from 'react'

import { Icon, type IconName } from './icons'
import { Tooltip } from './overlays'

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger'

export const Button = forwardRef<
  HTMLButtonElement,
  ButtonHTMLAttributes<HTMLButtonElement> & {
    variant?: ButtonVariant
    size?: 'sm' | 'md' | 'lg'
    icon?: IconName
    iconEnd?: IconName
    busy?: boolean
  }
>(function Button(
  { busy = false, children, className, disabled, icon, iconEnd, size = 'md', type = 'button', variant = 'secondary', ...props },
  ref,
) {
  const iconSize = size === 'sm' ? 14 : 16
  return (
    <button
      aria-busy={busy || undefined}
      className={['aw-btn', `aw-btn--${variant}`, `aw-btn--${size}`, className].filter(Boolean).join(' ')}
      disabled={disabled || busy}
      ref={ref}
      type={type}
      {...props}
    >
      {busy ? <Icon className="aw-spin" name="loader" size={iconSize} /> : icon ? <Icon name={icon} size={iconSize} /> : null}
      {children != null ? <span className="aw-btn__label">{children}</span> : null}
      {iconEnd ? <Icon name={iconEnd} size={iconSize} /> : null}
    </button>
  )
})

/** Square icon-only button. Its label is both the accessible name and the tooltip. */
export const IconButton = forwardRef<
  HTMLButtonElement,
  ButtonHTMLAttributes<HTMLButtonElement> & {
    icon: IconName
    label: string
    size?: 'sm' | 'md'
    pressed?: boolean
    tooltip?: boolean
    shortcut?: string
    side?: 'top' | 'bottom' | 'right'
    badge?: ReactNode
  }
>(function IconButton(
  { badge, className, icon, label, pressed, shortcut, side = 'top', size = 'md', tooltip = true, type = 'button', ...props },
  ref,
) {
  const button = (
    <button
      aria-label={label}
      aria-pressed={pressed}
      className={['aw-iconbtn', `aw-iconbtn--${size}`, className].filter(Boolean).join(' ')}
      ref={ref}
      type={type}
      {...props}
    >
      <Icon name={icon} size={size === 'sm' ? 16 : 18} />
      {badge}
    </button>
  )
  return tooltip ? (
    <Tooltip label={label} shortcut={shortcut} side={side}>
      {button}
    </Tooltip>
  ) : (
    button
  )
})

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="aw-kbd">{children}</kbd>
}

type FieldProps = {
  label?: ReactNode
  hint?: ReactNode
  error?: ReactNode
  prefix?: ReactNode
  mono?: boolean
}

function describedBy(id: string, hint: unknown, error: unknown) {
  return hint || error ? `${id}-note` : undefined
}

function FieldNote({ error, hint, id }: { error?: ReactNode; hint?: ReactNode; id: string }) {
  if (!error && !hint) return null
  return (
    <p className="aw-field__note" id={`${id}-note`} role={error ? 'alert' : undefined}>
      {error ? <Icon name="circle-alert" size={14} /> : null}
      {error || hint}
    </p>
  )
}

export const TextField = forwardRef<HTMLInputElement, Omit<InputHTMLAttributes<HTMLInputElement>, 'prefix'> & FieldProps>(
  function TextField({ className, error, hint, id: given, label, mono, prefix, ...input }, ref) {
    const generated = useId()
    const id = given ?? generated
    return (
      <div className={['aw-field', error ? 'is-invalid' : '', className].filter(Boolean).join(' ')}>
        {label ? (
          <label className="aw-field__label" htmlFor={id}>
            {label}
          </label>
        ) : null}
        <div className="aw-field__control">
          {prefix ? <span className="aw-field__prefix">{prefix}</span> : null}
          <input
            aria-describedby={describedBy(id, hint, error)}
            aria-invalid={error ? true : undefined}
            className={mono ? 'is-mono' : undefined}
            id={id}
            ref={ref}
            {...input}
          />
        </div>
        <FieldNote error={error} hint={hint} id={id} />
      </div>
    )
  },
)

export const TextArea = forwardRef<HTMLTextAreaElement, Omit<TextareaHTMLAttributes<HTMLTextAreaElement>, 'prefix'> & Omit<FieldProps, 'prefix' | 'mono'>>(
  function TextArea({ className, error, hint, id: given, label, ...input }, ref) {
    const generated = useId()
    const id = given ?? generated
    return (
      <div className={['aw-field', error ? 'is-invalid' : '', className].filter(Boolean).join(' ')}>
        {label ? (
          <label className="aw-field__label" htmlFor={id}>
            {label}
          </label>
        ) : null}
        <div className="aw-field__control aw-field__control--area">
          <textarea aria-describedby={describedBy(id, hint, error)} aria-invalid={error ? true : undefined} id={id} ref={ref} {...input} />
        </div>
        <FieldNote error={error} hint={hint} id={id} />
      </div>
    )
  },
)

export const SelectField = forwardRef<HTMLSelectElement, Omit<SelectHTMLAttributes<HTMLSelectElement>, 'prefix'> & Omit<FieldProps, 'mono'>>(
  function SelectField({ children, className, error, hint, id: given, label, prefix, ...input }, ref) {
    const generated = useId()
    const id = given ?? generated
    return (
      <div className={['aw-field', error ? 'is-invalid' : '', className].filter(Boolean).join(' ')}>
        {label ? (
          <label className="aw-field__label" htmlFor={id}>
            {label}
          </label>
        ) : null}
        <div className="aw-field__control aw-field__control--select">
          {prefix ? <span className="aw-field__prefix">{prefix}</span> : null}
          <select aria-describedby={describedBy(id, hint, error)} id={id} ref={ref} {...input}>
            {children}
          </select>
          <Icon name="chevron-down" size={14} />
        </div>
        <FieldNote error={error} hint={hint} id={id} />
      </div>
    )
  },
)

/** An on/off setting that applies immediately. */
export function Switch({
  checked,
  description,
  disabled,
  label,
  onChange,
}: {
  checked: boolean
  description?: ReactNode
  disabled?: boolean
  label: ReactNode
  onChange: (checked: boolean) => void
}) {
  const id = useId()
  return (
    <div className="aw-switch-row">
      <div className="aw-switch-row__text">
        <label className="aw-switch-row__label" htmlFor={id}>
          {label}
        </label>
        {description ? (
          <p className="aw-switch-row__desc" id={`${id}-d`}>
            {description}
          </p>
        ) : null}
      </div>
      <button
        aria-checked={checked}
        aria-describedby={description ? `${id}-d` : undefined}
        className="aw-switch"
        disabled={disabled}
        id={id}
        onClick={() => onChange(!checked)}
        role="switch"
        type="button"
      >
        <span className="aw-switch__thumb" />
      </button>
    </div>
  )
}

/** A checkbox with a visible label: a choice that takes effect when the form is saved or the box is ticked. */
export function Checkbox({
  checked,
  description,
  disabled,
  id: given,
  label,
  onChange,
}: {
  checked: boolean
  description?: ReactNode
  disabled?: boolean
  id?: string
  label: ReactNode
  onChange: (checked: boolean) => void
}) {
  const generated = useId()
  const id = given ?? generated
  return (
    <div className="aw-check">
      <input
        aria-describedby={description ? `${id}-d` : undefined}
        checked={checked}
        className="aw-check__input"
        disabled={disabled}
        id={id}
        onChange={(event) => onChange(event.target.checked)}
        type="checkbox"
      />
      <span aria-hidden="true" className="aw-check__box">
        <Icon name="check" size={12} />
      </span>
      <span className="aw-check__text">
        <label className="aw-check__label" htmlFor={id}>
          {label}
        </label>
        {description ? (
          <span className="aw-check__desc" id={`${id}-d`}>
            {description}
          </span>
        ) : null}
      </span>
    </div>
  )
}

export function SegmentedControl<T extends string>({
  label,
  onChange,
  options,
  value,
}: {
  label: string
  onChange: (value: T) => void
  options: { value: T; label: string; icon?: IconName }[]
  value: T
}) {
  const group = useRef<HTMLDivElement>(null)
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step =
      event.key === 'ArrowRight' || event.key === 'ArrowDown' ? 1 : event.key === 'ArrowLeft' || event.key === 'ArrowUp' ? -1 : 0
    if (!step) return
    event.preventDefault()
    const index = options.findIndex((option) => option.value === value)
    const next = options[(index + step + options.length) % options.length]
    onChange(next.value)
    group.current?.querySelector<HTMLElement>(`[data-value="${next.value}"]`)?.focus()
  }
  return (
    <div aria-label={label} className="aw-seg" onKeyDown={onKeyDown} ref={group} role="radiogroup">
      {options.map((option) => (
        <button
          aria-checked={option.value === value}
          className="aw-seg__opt"
          data-value={option.value}
          key={option.value}
          onClick={() => onChange(option.value)}
          role="radio"
          tabIndex={option.value === value ? 0 : -1}
          type="button"
        >
          {option.icon ? <Icon name={option.icon} size={14} /> : null}
          {option.label}
        </button>
      ))}
    </div>
  )
}
