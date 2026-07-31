import { KeyboardEvent, RefObject, useEffect, useRef, useState } from 'react'

import type { ConversationActor } from './types'

type MentionPickerProps = {
  actors: ConversationActor[]
  busy: boolean
  error: string | null
  onClose: () => void
  onQueryChange: (query: string) => void
  onSelect: (actor: ConversationActor) => void
  returnFocusRef: RefObject<HTMLElement | null>
}

export function MentionPicker({
  actors,
  busy,
  error,
  onClose,
  onQueryChange,
  onSelect,
  returnFocusRef,
}: MentionPickerProps) {
  const inputRef = useRef<HTMLInputElement>(null)
  const [activeIndex, setActiveIndex] = useState(0)

  useEffect(() => {
    inputRef.current?.focus()
  }, [])

  const close = () => {
    onClose()
    globalThis.setTimeout(() => returnFocusRef.current?.focus(), 0)
  }

  const select = (actor: ConversationActor) => {
    onSelect(actor)
    close()
  }

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault()
      close()
      return
    }
    if (event.key === 'ArrowDown' && actors.length > 0) {
      event.preventDefault()
      setActiveIndex((index) => (index + 1) % actors.length)
      return
    }
    if (event.key === 'ArrowUp' && actors.length > 0) {
      event.preventDefault()
      setActiveIndex((index) => (index - 1 + actors.length) % actors.length)
      return
    }
    if (event.key === 'Enter' && actors[activeIndex]) {
      event.preventDefault()
      select(actors[activeIndex])
    }
  }

  return (
    <section aria-label="Mention an Actor" className="mention-picker">
      <header>
        <div>
          <span>Insert mention</span>
          <h3>Mention an Actor</h3>
        </div>
        <button aria-label="Close mention picker" onClick={close} type="button">
          ×
        </button>
      </header>
      <label htmlFor="mention-search">Search Project members</label>
      <input
        aria-activedescendant={
          actors[activeIndex] ? `mention-option-${actors[activeIndex].id}` : undefined
        }
        aria-controls="mention-results"
        aria-expanded="true"
        autoComplete="off"
        id="mention-search"
        onChange={(event) => onQueryChange(event.target.value)}
        onKeyDown={onKeyDown}
        ref={inputRef}
        role="combobox"
      />
      <p aria-live="polite" className="visually-hidden">
        {busy
          ? 'Searching Project members.'
          : error
            ? error
            : `${actors.length} mentionable Actors available.`}
      </p>
      {error ? <p role="alert">{error}</p> : null}
      {!error && !busy ? (
        <div aria-label="Mentionable Actors" id="mention-results" role="listbox">
          {actors.length === 0 ? (
            <p>No mentionable Actors found.</p>
          ) : (
            actors.map((actor, index) => (
              <button
                aria-selected={index === activeIndex}
                id={`mention-option-${actor.id}`}
                key={actor.id}
                onClick={() => select(actor)}
                onMouseEnter={() => setActiveIndex(index)}
                role="option"
                type="button"
              >
                <strong>{actor.displayName}</strong>
                <span>@{actor.handle}</span>
                <span>{actor.kind === 'execution_worker' ? 'Worker' : 'Human'}</span>
              </button>
            ))
          )}
        </div>
      ) : null}
    </section>
  )
}
