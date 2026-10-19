import {
  cloneElement,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type FormEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
  type ReactNode,
  type RefObject,
} from 'react'
import { createPortal } from 'react-dom'

import { Icon, type IconName } from './icons'

export type Placement = 'top' | 'bottom' | 'right' | 'bottom-start' | 'bottom-end' | 'top-start' | 'right-start'

const GAP = 6

/** `element.matches`, treating a selector the browser can't parse as a match. */
function safeMatches(element: Element, selector: string) {
  try {
    return element.matches(selector)
  } catch {
    return true
  }
}

function place(anchor: HTMLElement, floating: HTMLElement | null, placement: Placement) {
  const a = anchor.getBoundingClientRect()
  const f = floating?.getBoundingClientRect()
  const width = f?.width ?? 0
  const height = f?.height ?? 0
  let top = a.bottom + GAP
  let left = a.left
  if (placement === 'top' || placement === 'top-start') top = a.top - height - GAP
  if (placement === 'right' || placement === 'right-start') {
    left = a.right + GAP + 4
    top = placement === 'right' ? a.top + a.height / 2 - height / 2 : a.top
  }
  if (placement === 'top' || placement === 'bottom') left = a.left + a.width / 2 - width / 2
  if (placement === 'bottom-end') left = a.right - width
  // Flip below an anchor near the top, and above one near the bottom.
  if ((placement === 'top' || placement === 'top-start') && top < 4) top = a.bottom + GAP
  if (placement.startsWith('bottom') && top + height > window.innerHeight - 4 && a.top - height - GAP > 4) {
    top = a.top - height - GAP
  }
  left = Math.max(4, Math.min(left, window.innerWidth - width - 4))
  top = Math.max(4, Math.min(top, window.innerHeight - height - 4))
  return { top, left }
}

/** Fixed-position coordinates next to an anchor, kept in the viewport and updated on scroll. */
function useAnchoredPosition(
  anchor: HTMLElement | null,
  floating: RefObject<HTMLElement | null>,
  open: boolean,
  placement: Placement,
) {
  const [position, setPosition] = useState<{ top: number; left: number } | null>(null)
  useLayoutEffect(() => {
    if (!open || !anchor) return undefined
    const update = () => setPosition(place(anchor, floating.current, placement))
    // Measure after the floating element has rendered once, so its size is known.
    const frame = window.requestAnimationFrame(update)
    window.addEventListener('resize', update)
    window.addEventListener('scroll', update, true)
    return () => {
      window.cancelAnimationFrame(frame)
      window.removeEventListener('resize', update)
      window.removeEventListener('scroll', update, true)
    }
  }, [anchor, floating, open, placement])
  return open ? position : null
}

type TriggerProps = {
  'aria-describedby'?: string
}

/** A short label on hover or focus. The trigger keeps its own accessible name. */
export function Tooltip({
  children,
  label,
  shortcut,
  side = 'top',
}: {
  children: ReactElement<TriggerProps>
  label: string
  shortcut?: string
  side?: 'top' | 'bottom' | 'right'
}) {
  const [open, setOpen] = useState(false)
  const [anchor, setAnchor] = useState<HTMLSpanElement | null>(null)
  const bubble = useRef<HTMLSpanElement>(null)
  const timer = useRef<number | undefined>(undefined)
  const position = useAnchoredPosition(anchor, bubble, open, side)
  const show = () => {
    window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => setOpen(true), 350)
  }
  const hide = () => {
    window.clearTimeout(timer.current)
    setOpen(false)
  }
  useEffect(() => () => window.clearTimeout(timer.current), [])
  return (
    <span
      className="aw-tip"
      onBlur={hide}
      onFocus={(event) => {
        // Keyboard focus only: a click focuses the button too, and the label
        // popping up right after the click is noise.
        const target = event.target as HTMLElement
        if (typeof target.matches === 'function' && !safeMatches(target, ':focus-visible')) return
        window.clearTimeout(timer.current)
        setOpen(true)
      }}
      onKeyDown={(event) => event.key === 'Escape' && hide()}
      onMouseDown={hide}
      onMouseEnter={show}
      onMouseLeave={hide}
      ref={setAnchor}
    >
      {children}
      {open
        ? createPortal(
            <span
              aria-hidden="true"
              className={`aw-tooltip aw-tooltip--${side}`}
              ref={bubble}
              style={position ? { top: position.top, left: position.left } : { visibility: 'hidden' }}
            >
              {label}
              {shortcut ? <kbd>{shortcut}</kbd> : null}
            </span>,
            document.body,
          )
        : null}
    </span>
  )
}

export type MenuEntry =
  | {
      label: string
      icon?: IconName
      shortcut?: string
      danger?: boolean
      /** Makes it a checkbox item: ✓ when true. */
      checked?: boolean
      disabled?: boolean
      /** Leaves the menu open after choosing, for toggling several checkbox items in a row. */
      keepOpen?: boolean
      onSelect: () => void
    }
  | { separator: true }
  | { heading: string }

type MenuTriggerProps = {
  'aria-controls'?: string
  'aria-expanded'?: boolean
  'aria-haspopup'?: 'menu'
  onClick?: (event: ReactMouseEvent<HTMLElement>) => void
  ref?: (element: HTMLElement | null) => void
}

/** A button-anchored action menu with arrow-key navigation, rendered in a portal. */
export function Menu({
  items,
  label,
  placement = 'bottom-start',
  trigger,
}: {
  items: MenuEntry[]
  label: string
  placement?: Placement
  trigger: ReactElement<MenuTriggerProps>
}) {
  const [open, setOpen] = useState(false)
  const [anchor, setAnchor] = useState<HTMLElement | null>(null)
  const panel = useRef<HTMLDivElement>(null)
  const id = useId()
  const position = useAnchoredPosition(anchor, panel, open, placement)

  const close = useCallback(
    (returnFocus = true) => {
      setOpen(false)
      if (returnFocus) window.requestAnimationFrame(() => anchor?.focus())
    },
    [anchor],
  )

  useEffect(() => {
    if (!open) return undefined
    const onPointer = (event: MouseEvent) => {
      const target = event.target as Node
      if (panel.current?.contains(target) || anchor?.contains(target)) return
      close(false)
    }
    document.addEventListener('mousedown', onPointer)
    return () => document.removeEventListener('mousedown', onPointer)
  }, [anchor, close, open])

  const focusedOnOpen = useRef(false)
  useEffect(() => {
    if (!open) {
      focusedOnOpen.current = false
      return
    }
    if (position && !focusedOnOpen.current) {
      focusedOnOpen.current = true
      panel.current?.querySelector<HTMLElement>('[role^="menuitem"]:not([disabled])')?.focus()
    }
  }, [open, position])

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const entries = [...(panel.current?.querySelectorAll<HTMLElement>('[role^="menuitem"]:not([disabled])') ?? [])]
    const index = entries.indexOf(document.activeElement as HTMLElement)
    const go = (next: number) => {
      event.preventDefault()
      entries[(next + entries.length) % entries.length]?.focus()
    }
    if (event.key === 'ArrowDown') go(index + 1)
    else if (event.key === 'ArrowUp') go(index - 1)
    else if (event.key === 'Home') go(0)
    else if (event.key === 'End') go(entries.length - 1)
    else if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      close()
    } else if (event.key === 'Tab') close(false)
  }

  const triggerElement = cloneElement(trigger, {
    'aria-controls': open ? id : undefined,
    'aria-expanded': open,
    'aria-haspopup': 'menu',
    onClick: (event: ReactMouseEvent<HTMLElement>) => {
      trigger.props.onClick?.(event)
      setOpen((current) => !current)
    },
    ref: setAnchor,
  })

  return (
    <>
      {triggerElement}
      {open
        ? createPortal(
            <div
              aria-label={label}
              className="aw-menu"
              id={id}
              onKeyDown={onKeyDown}
              ref={panel}
              role="menu"
              style={position ? { top: position.top, left: position.left } : { visibility: 'hidden' }}
            >
              {items.map((item, index) => {
                if ('separator' in item) return <div className="aw-menu__sep" key={index} role="separator" />
                if ('heading' in item) {
                  return (
                    <div className="aw-menu__heading" key={index} role="presentation">
                      {item.heading}
                    </div>
                  )
                }
                const checkable = item.checked !== undefined
                return (
                  <button
                    aria-checked={checkable ? item.checked : undefined}
                    className={item.danger ? 'aw-menu__item is-danger' : 'aw-menu__item'}
                    disabled={item.disabled}
                    key={index}
                    onClick={() => {
                      if (item.keepOpen) {
                        item.onSelect()
                        return
                      }
                      // Focus goes back to the trigger before the action runs, so a
                      // dialog it opens remembers the trigger and returns focus there.
                      setOpen(false)
                      anchor?.focus()
                      item.onSelect()
                    }}
                    role={checkable ? 'menuitemcheckbox' : 'menuitem'}
                    tabIndex={-1}
                    title={item.label.length > 40 ? item.label : undefined}
                    type="button"
                  >
                    {item.icon ? <Icon name={item.icon} /> : null}
                    <span className="aw-menu__label">{item.label}</span>
                    {item.shortcut ? <kbd>{item.shortcut}</kbd> : null}
                    {item.checked ? <Icon className="aw-menu__check" name="check" /> : null}
                  </button>
                )
              })}
            </div>,
            document.body,
          )
        : null}
    </>
  )
}

const focusableSelector =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'

/** A modal for one short task. Traps focus, closes on Escape, restores focus on close. */
export function Dialog({
  children,
  description,
  dirty = false,
  footer,
  onClose,
  onSubmit,
  open,
  returnFocus,
  title,
  width = 480,
}: {
  children?: ReactNode
  description?: ReactNode
  /** Something typed would be lost: a click outside doesn't close it (Escape, Cancel and ✕ still do). */
  dirty?: boolean
  footer?: ReactNode
  onClose: () => void
  /** Makes the body and footer one form, so the footer's submit button sends it. */
  onSubmit?: (event: FormEvent<HTMLFormElement>) => void
  open: boolean
  /** Where focus goes on close. Defaults to whatever had focus when it opened. */
  returnFocus?: RefObject<HTMLElement | null>
  title: string
  width?: number
}) {
  const panel = useRef<HTMLDivElement>(null)
  const restore = useRef<HTMLElement | null>(null)
  const returnRef = useRef(returnFocus)
  const closeRef = useRef(onClose)
  const dirtyRef = useRef(dirty)
  const titleId = useId()
  const descriptionId = useId()
  useEffect(() => {
    closeRef.current = onClose
    returnRef.current = returnFocus
    dirtyRef.current = dirty
  }, [dirty, onClose, returnFocus])

  useEffect(() => {
    if (!open) return undefined
    restore.current = document.activeElement as HTMLElement | null
    const root = document.querySelector<HTMLElement>('#root')
    const rootWasInert = root?.hasAttribute('inert') ?? false
    root?.setAttribute('inert', '')
    const first =
      panel.current?.querySelector<HTMLElement>('[data-autofocus]') ??
      panel.current?.querySelector<HTMLElement>('.aw-dialog__body input:not([type="hidden"]):not([disabled]), .aw-dialog__body textarea:not([disabled]), .aw-dialog__body select:not([disabled])') ??
      panel.current?.querySelector<HTMLElement>('.aw-dialog__foot .aw-btn:not(.aw-btn--danger):not([disabled])') ??
      panel.current?.querySelector<HTMLElement>(focusableSelector)
    first?.focus()
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        closeRef.current()
        return
      }
      if (event.key !== 'Tab' || !panel.current) return
      const controls = [...panel.current.querySelectorAll<HTMLElement>(focusableSelector)]
      if (!controls.length) return
      const firstControl = controls[0]
      const lastControl = controls[controls.length - 1]
      if (event.shiftKey && document.activeElement === firstControl) {
        event.preventDefault()
        lastControl.focus()
      } else if (!event.shiftKey && document.activeElement === lastControl) {
        event.preventDefault()
        firstControl.focus()
      }
    }
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('keydown', onKey)
      if (!rootWasInert) root?.removeAttribute('inert')
      const target = returnRef.current?.current ?? restore.current
      window.requestAnimationFrame(() => target?.focus())
    }
  }, [open])

  if (!open) return null
  return createPortal(
    <div className="aw-dialog-layer">
      <div
        className="aw-scrim"
        onMouseDown={() => {
          if (!dirtyRef.current) closeRef.current()
        }}
      />
      <div
        aria-describedby={description ? descriptionId : undefined}
        aria-labelledby={titleId}
        aria-modal="true"
        className="aw-dialog"
        ref={panel}
        role="dialog"
        style={{ width }}
      >
        <header className="aw-dialog__head">
          <h2 className="aw-dialog__title" id={titleId}>
            {title}
          </h2>
          <button aria-label="Close" className="aw-iconbtn aw-iconbtn--sm" onClick={() => closeRef.current()} type="button">
            <Icon name="x" />
          </button>
        </header>
        {description ? (
          <p className="aw-dialog__desc" id={descriptionId}>
            {description}
          </p>
        ) : null}
        {onSubmit ? (
          <form className="aw-dialog__form" noValidate onSubmit={onSubmit}>
            <div className="aw-dialog__body">{children}</div>
            {footer ? <footer className="aw-dialog__foot">{footer}</footer> : null}
          </form>
        ) : (
          <>
            <div className="aw-dialog__body">{children}</div>
            {footer ? <footer className="aw-dialog__foot">{footer}</footer> : null}
          </>
        )}
      </div>
    </div>,
    document.body,
  )
}
