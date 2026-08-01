import { KeyboardEvent, RefObject, useEffect, useRef, useState } from 'react'

import type { ConversationWorkItemReference } from './types'

export function WorkItemPicker({
  busy,
  error,
  items,
  onClose,
  onQueryChange,
  onSelect,
  returnFocusRef,
}: {
  busy: boolean
  error: string | null
  items: ConversationWorkItemReference[]
  onClose: () => void
  onQueryChange: (query: string) => void
  onSelect: (item: ConversationWorkItemReference) => void
  returnFocusRef: RefObject<HTMLElement | null>
}) {
  const inputRef = useRef<HTMLInputElement>(null)
  const [activeIndex, setActiveIndex] = useState(0)
  useEffect(() => inputRef.current?.focus(), [])
  const close = () => {
    onClose()
    globalThis.setTimeout(() => returnFocusRef.current?.focus(), 0)
  }
  const select = (item: ConversationWorkItemReference) => {
    onSelect(item)
    close()
  }
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Escape') { event.preventDefault(); close() }
    else if (event.key === 'ArrowDown' && items.length) { event.preventDefault(); setActiveIndex((index) => (index + 1) % items.length) }
    else if (event.key === 'ArrowUp' && items.length) { event.preventDefault(); setActiveIndex((index) => (index - 1 + items.length) % items.length) }
    else if (event.key === 'Enter' && items[activeIndex]) { event.preventDefault(); select(items[activeIndex]) }
  }
  return (
    <section aria-label="Reference a Work Item" className="mention-picker">
      <header><div><span>Insert reference</span><h3>Reference a Work Item</h3></div><button aria-label="Close Work Item picker" onClick={close} type="button">×</button></header>
      <label htmlFor="work-item-reference-search">Search key or title</label>
      <input
        aria-activedescendant={items[activeIndex] ? `work-item-option-${items[activeIndex].id}` : undefined}
        aria-controls="work-item-reference-results"
        aria-expanded="true"
        autoComplete="off"
        id="work-item-reference-search"
        onChange={(event) => onQueryChange(event.target.value)}
        onKeyDown={onKeyDown}
        ref={inputRef}
        role="combobox"
      />
      <p aria-live="polite" className="visually-hidden">{busy ? 'Searching Work Items.' : error ?? `${items.length} Work Items available.`}</p>
      {error ? <p role="alert">{error}</p> : null}
      {!error && !busy ? <div aria-label="Work Items" id="work-item-reference-results" role="listbox">{items.length ? items.map((item, index) => <button aria-selected={index === activeIndex} id={`work-item-option-${item.id}`} key={item.id} onClick={() => select(item)} onMouseEnter={() => setActiveIndex(index)} role="option" type="button"><strong>{item.key}</strong><span>{item.title}</span><span>{item.archivedAt ? 'Archived' : item.statusCategory}</span></button>) : <p>No Work Items found.</p>}</div> : null}
    </section>
  )
}
