import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { createPortal } from 'react-dom'

import { filterCommands, type CommandItem } from './commandModel'
import { Icon } from './icons'

function grouped(items: CommandItem[]) {
  const groups: { name: string; items: CommandItem[] }[] = []
  for (const item of items) {
    const group = groups.find((candidate) => candidate.name === item.group)
    if (group) group.items.push(item)
    else groups.push({ name: item.group, items: [item] })
  }
  return groups
}

/**
 * A searchable list of things to go to and do, as a modal. Arrow keys move,
 * Enter runs the highlighted command, Escape closes. The caller supplies the
 * commands and may add search results as the query changes.
 */
export function CommandMenu({
  emptyText = 'Nothing matches. Try another word.',
  footer,
  items,
  label,
  loading = false,
  onClose,
  onQueryChange,
  placeholder = 'Search or type a command',
  query: controlledQuery,
}: {
  emptyText?: string
  footer?: ReactNode
  items: CommandItem[]
  label: string
  loading?: boolean
  onClose: () => void
  onQueryChange?: (query: string) => void
  placeholder?: string
  query?: string
}) {
  const [ownQuery, setOwnQuery] = useState('')
  const query = controlledQuery ?? ownQuery
  const [activeIndex, setActiveIndex] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const restore = useRef<HTMLElement | null>(null)
  const listId = useId()
  const headingId = useId()
  const visible = useMemo(() => filterCommands(items, query), [items, query])
  const groups = useMemo(() => grouped(visible), [visible])
  // Keyboard order follows the groups as shown.
  const ordered = useMemo(() => groups.flatMap((group) => group.items), [groups])
  const active = ordered[Math.min(activeIndex, ordered.length - 1)]

  useEffect(() => {
    restore.current = document.activeElement as HTMLElement | null
    inputRef.current?.focus()
    const root = document.querySelector<HTMLElement>('#root')
    const rootWasInert = root?.hasAttribute('inert') ?? false
    root?.setAttribute('inert', '')
    return () => {
      if (!rootWasInert) root?.removeAttribute('inert')
      const target = restore.current
      window.requestAnimationFrame(() => target?.isConnected && target.focus())
    }
  }, [])

  useEffect(() => {
    if (!active) return
    const options = listRef.current?.querySelectorAll<HTMLElement>('[data-command-id]') ?? []
    for (const option of options) {
      if (option.dataset.commandId === active.id) option.scrollIntoView?.({ block: 'nearest' })
    }
  }, [active])

  const setQuery = (value: string) => {
    setActiveIndex(0)
    if (controlledQuery === undefined) setOwnQuery(value)
    onQueryChange?.(value)
  }

  const run = (item: CommandItem | undefined) => {
    if (!item) return
    // Let the menu give focus back before the command moves it somewhere new.
    restore.current = null
    onClose()
    item.onSelect()
  }

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const count = ordered.length
    const move = (next: number) => {
      event.preventDefault()
      if (count) setActiveIndex((next + count) % count)
    }
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      onClose()
    } else if (event.key === 'ArrowDown') move(activeIndex + 1)
    else if (event.key === 'ArrowUp') move(activeIndex - 1)
    else if (event.key === 'Home' && event.ctrlKey) move(0)
    else if (event.key === 'End' && event.ctrlKey) move(count - 1)
    else if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
      event.preventDefault()
      run(active)
    } else if (event.key === 'Tab') {
      // The input is the only stop; keep focus inside the menu.
      event.preventDefault()
    }
  }

  const optionId = (item: CommandItem) => `${listId}-${item.id}`

  return createPortal(
    <div
      className="aw-dialog-layer aw-dialog-layer--top"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target) onClose()
      }}
    >
      <div aria-hidden="true" className="aw-scrim" />
      <div aria-labelledby={headingId} aria-modal="true" className="aw-dialog aw-cmdk" onKeyDown={onKeyDown} role="dialog">
        <h2 className="visually-hidden" id={headingId}>
          {label}
        </h2>
        <div className="aw-cmdk__search">
          <Icon name="search" size={18} />
          <input
            aria-activedescendant={active ? optionId(active) : undefined}
            aria-autocomplete="list"
            aria-controls={listId}
            aria-expanded="true"
            aria-label={label}
            autoComplete="off"
            maxLength={200}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={placeholder}
            ref={inputRef}
            role="combobox"
            spellCheck={false}
            type="text"
            value={query}
          />
          {loading ? <Icon className="aw-spin aw-cmdk__loading" name="loader" size={16} /> : null}
          <kbd className="aw-cmdk__esc">Esc</kbd>
        </div>
        <div aria-busy={loading} aria-label={label} className="aw-cmdk__list" id={listId} ref={listRef} role="listbox">
          {groups.map((group) => (
            <div aria-label={group.name} className="aw-cmdk__group" key={group.name} role="group">
              <div aria-hidden="true" className="aw-cmdk__heading">
                {group.name}
              </div>
              {group.items.map((item) => {
                const selected = item === active
                return (
                  <div
                    aria-selected={selected}
                    className={`aw-cmdk__item${selected ? ' is-active' : ''}`}
                    data-command-id={item.id}
                    id={optionId(item)}
                    key={item.id}
                    onClick={() => run(item)}
                    onMouseDown={(event) => event.preventDefault()}
                    onMouseMove={() => {
                      const index = ordered.indexOf(item)
                      if (index !== activeIndex) setActiveIndex(index)
                    }}
                    role="option"
                  >
                    <span aria-hidden="true" className="aw-cmdk__mark">
                      {item.mark ?? (item.icon ? <Icon name={item.icon} size={16} /> : null)}
                    </span>
                    <span className="aw-cmdk__label">{item.label}</span>
                    {item.hint ? <span className="aw-cmdk__hint">{item.hint}</span> : null}
                    {item.shortcut ? <kbd className="aw-cmdk__kbd">{item.shortcut}</kbd> : null}
                  </div>
                )
              })}
            </div>
          ))}
          {!ordered.length && !loading ? (
            <div className="aw-cmdk__empty" role="presentation">
              {emptyText}
            </div>
          ) : null}
        </div>
        {footer ? <div className="aw-cmdk__footer">{footer}</div> : null}
      </div>
    </div>,
    document.body,
  )
}
