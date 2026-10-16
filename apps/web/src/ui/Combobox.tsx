import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'

import { Icon } from './icons'
import { Popover } from './Popover'
import type { PopoverPlacement } from './popoverModel'

export type ComboboxOption = {
  value: string
  label: string
  /** Shown under or beside the label, such as a handle. */
  description?: string
  leading?: ReactNode
  /** Extra words that match the search, such as a handle or key. */
  keywords?: string
  disabled?: boolean
}

function matches(option: ComboboxOption, query: string) {
  if (!query) return true
  const haystack = `${option.label} ${option.description ?? ''} ${option.keywords ?? ''}`.toLocaleLowerCase()
  return query
    .toLocaleLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => haystack.includes(word))
}

/**
 * A search field over a list of options, for use inside a popover or a page.
 * Arrow keys move, Enter picks, Escape is left to the container. With
 * `multiple`, Enter and clicks toggle and the list stays open.
 */
export function ComboboxList({
  createLabel = (query) => `Create “${query}”`,
  emptyText = 'No matches',
  footer,
  label,
  loading = false,
  multiple = false,
  onClose,
  onCreate,
  onQueryChange,
  onSelect,
  options,
  placeholder = 'Search',
  selected,
}: {
  /** The words on the extra option `onCreate` adds. */
  createLabel?: (query: string) => string
  emptyText?: string
  /** Extra actions under the list, such as "Manage labels". */
  footer?: ReactNode
  label: string
  loading?: boolean
  multiple?: boolean
  /** Called after a single pick, so a popover can close. */
  onClose?: () => void
  /** Offers "Create …" for a query that matches nothing exactly. */
  onCreate?: (query: string) => void
  /** For server search: the options then come back already filtered. */
  onQueryChange?: (query: string) => void
  onSelect: (value: string) => void
  options: ComboboxOption[]
  placeholder?: string
  selected: readonly string[]
}) {
  const [query, setQuery] = useState('')
  const [active, setActive] = useState(0)
  const listId = useId()
  const listRef = useRef<HTMLUListElement>(null)
  const filtered = useMemo(
    () => (onQueryChange ? options : options.filter((option) => matches(option, query.trim()))),
    [onQueryChange, options, query],
  )
  const trimmed = query.trim()
  const canCreate = Boolean(onCreate && trimmed && !options.some((option) => option.label.toLocaleLowerCase() === trimmed.toLocaleLowerCase()))
  const count = filtered.length + (canCreate ? 1 : 0)
  const activeIndex = Math.min(active, Math.max(0, count - 1))

  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${activeIndex}"]`)?.scrollIntoView?.({ block: 'nearest' })
  }, [activeIndex])

  const choose = (index: number) => {
    if (index < filtered.length) {
      const option = filtered[index]
      if (!option || option.disabled) return
      onSelect(option.value)
      if (!multiple) onClose?.()
    } else if (canCreate) {
      onCreate?.(trimmed)
      setQuery('')
    }
  }

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return
    const move = (next: number) => {
      event.preventDefault()
      if (count) setActive((next + count) % count)
    }
    if (event.key === 'ArrowDown') move(activeIndex + 1)
    else if (event.key === 'ArrowUp') move(activeIndex - 1)
    else if (event.key === 'Home' && event.ctrlKey) move(0)
    else if (event.key === 'End' && event.ctrlKey) move(count - 1)
    else if (event.key === 'Enter') {
      event.preventDefault()
      if (count) choose(activeIndex)
    } else if (event.key === 'Tab' && onClose) {
      onClose()
    }
  }

  const optionId = (index: number) => `${listId}-${index}`
  return (
    <div className="aw-combobox">
      <div className="aw-combobox__search">
        <Icon name="search" size={14} />
        <input
          aria-activedescendant={count ? optionId(activeIndex) : undefined}
          aria-autocomplete="list"
          aria-controls={listId}
          aria-expanded="true"
          aria-label={label}
          autoComplete="off"
          className="aw-combobox__input"
          onChange={(event) => {
            setQuery(event.target.value)
            setActive(0)
            onQueryChange?.(event.target.value)
          }}
          onKeyDown={onKeyDown}
          placeholder={placeholder}
          role="combobox"
          spellCheck={false}
          type="text"
          value={query}
        />
        {loading ? <Icon className="aw-spin" name="loader" size={14} /> : null}
      </div>
      <ul aria-label={label} aria-multiselectable={multiple || undefined} className="aw-combobox__list" id={listId} ref={listRef} role="listbox">
        {filtered.map((option, index) => {
          const isSelected = selected.includes(option.value)
          return (
            <li
              aria-disabled={option.disabled || undefined}
              aria-selected={isSelected}
              className={['aw-combobox__option', index === activeIndex ? 'is-active' : '', isSelected ? 'is-selected' : ''].filter(Boolean).join(' ')}
              data-index={index}
              id={optionId(index)}
              key={option.value}
              onClick={() => choose(index)}
              onMouseMove={() => index !== activeIndex && setActive(index)}
              role="option"
            >
              {multiple ? (
                <span aria-hidden="true" className="aw-combobox__box">
                  {isSelected ? <Icon name="check" size={12} /> : null}
                </span>
              ) : null}
              {option.leading ? <span className="aw-combobox__leading">{option.leading}</span> : null}
              <span className="aw-combobox__label">{option.label}</span>
              {option.description ? <span className="aw-combobox__desc">{option.description}</span> : null}
              {!multiple && isSelected ? <Icon className="aw-combobox__check" name="check" size={14} /> : null}
            </li>
          )
        })}
        {canCreate ? (
          <li
            aria-selected={false}
            className={['aw-combobox__option', activeIndex === filtered.length ? 'is-active' : ''].filter(Boolean).join(' ')}
            data-index={filtered.length}
            id={optionId(filtered.length)}
            onClick={() => choose(filtered.length)}
            role="option"
          >
            <Icon name="plus" size={14} />
            <span className="aw-combobox__label">{createLabel(trimmed)}</span>
          </li>
        ) : null}
        {count === 0 ? (
          <li aria-disabled="true" className="aw-combobox__empty" role="option" aria-selected={false}>
            {loading ? 'Searching…' : emptyText}
          </li>
        ) : null}
      </ul>
      {footer ? <div className="aw-combobox__footer">{footer}</div> : null}
    </div>
  )
}

/**
 * A button that opens a searchable list: one value, or several with
 * `multiple`. The button shows the current value.
 */
export function Combobox({
  children,
  className,
  disabled,
  emptyText,
  footer,
  label,
  loading,
  multiple = false,
  onChange,
  onQueryChange,
  options,
  placement,
  value,
}: {
  /** The button's content: usually the current value. */
  children: ReactNode
  className?: string
  disabled?: boolean
  emptyText?: string
  footer?: ReactNode
  label: string
  loading?: boolean
  multiple?: boolean
  onChange: (value: string[]) => void
  onQueryChange?: (query: string) => void
  options: ComboboxOption[]
  placement?: PopoverPlacement
  value: readonly string[]
}) {
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null)
  const button = useRef<HTMLButtonElement>(null)
  const close = () => setAnchor(null)
  return (
    <>
      <button
        aria-expanded={anchor !== null}
        aria-haspopup="dialog"
        aria-label={label}
        className={['aw-combobox-trigger', className].filter(Boolean).join(' ')}
        disabled={disabled}
        onClick={(event) => setAnchor(anchor ? null : event.currentTarget)}
        ref={button}
        type="button"
      >
        {children}
      </button>
      <Popover anchor={anchor} label={label} onClose={close} open={anchor !== null} placement={placement} width={280}>
        <ComboboxList
          emptyText={emptyText}
          footer={footer}
          label={label}
          loading={loading}
          multiple={multiple}
          onClose={close}
          onQueryChange={onQueryChange}
          onSelect={(picked) => {
            if (!multiple) onChange([picked])
            else onChange(value.includes(picked) ? value.filter((entry) => entry !== picked) : [...value, picked])
          }}
          options={options}
          selected={value}
        />
      </Popover>
    </>
  )
}
