import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
} from 'react'

import { ActorTag, Avatar, Button, Icon, IconButton, Kbd, actorKind, type IconName } from '../ui'
import {
  activeTrigger,
  buildMessageContent,
  composerScalarValues,
  createIdempotencyKey,
  MAX_MESSAGE_SCALAR_VALUES,
  type ComposerDraft,
  type ComposerSubmission,
  type Trigger,
} from './composerModel'
import type { ConversationActor, ConversationWorkItemReference } from './types'

type StructuredComposerProps = {
  disabled?: boolean
  /** Replaces the text box, for example "This channel is archived." */
  disabledReason?: ReactNode
  label?: string
  onSubmit: (submission: ComposerSubmission) => Promise<void>
  placeholder?: string
  searchActors: (query: string) => Promise<ConversationActor[]>
  searchWorkItems?: (query: string) => Promise<ConversationWorkItemReference[]>
  submitLabel?: string
  /** Controls above the text box, such as the thread's Message/Run switch. */
  toolbar?: ReactNode
  /** Start from this text, for example the message being edited. */
  initialDraft?: ComposerDraft
  /** Shows a Cancel button, and Escape cancels too. */
  onCancel?: () => void
  /** A line under the box that is always shown, in place of the keyboard hint. */
  note?: ReactNode
  /** Editing: keep the text in the box while it saves; the editor closes on success. */
  keepDraftWhileSaving?: boolean
  autoFocus?: boolean
  className?: string
  submitIcon?: IconName
}

type Suggestion =
  | { kind: 'mention'; actor: ConversationActor }
  | { kind: 'work'; item: ConversationWorkItemReference }

/** One rounded box: type, @mention people and agents inline, reference work items with #, Enter to send. */
export function StructuredComposer({
  disabled = false,
  disabledReason,
  label = 'Message',
  onSubmit,
  placeholder = 'Message',
  searchActors,
  searchWorkItems,
  submitLabel = 'Send message',
  toolbar,
  initialDraft,
  onCancel,
  note,
  keepDraftWhileSaving = false,
  autoFocus = false,
  className,
  submitIcon = 'send',
}: StructuredComposerProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const idempotencyKeyRef = useRef(createIdempotencyKey())
  const searchGeneration = useRef(0)
  const id = useId()
  const [text, setText] = useState(initialDraft?.text ?? '')
  const [mentions, setMentions] = useState<ConversationActor[]>(initialDraft?.mentions ?? [])
  const [workItems, setWorkItems] = useState<ConversationWorkItemReference[]>(initialDraft?.workItems ?? [])
  const [trigger, setTrigger] = useState<Trigger | null>(null)
  const [suggestions, setSuggestions] = useState<Suggestion[]>([])
  const [active, setActive] = useState(0)
  const [searching, setSearching] = useState(false)
  const [searchError, setSearchError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // Pages pass new search functions each render; read the latest without restarting the search.
  const searchers = useRef({ searchActors, searchWorkItems })
  useEffect(() => {
    searchers.current = { searchActors, searchWorkItems }
  })

  const scalarValues = composerScalarValues(text)
  const overLimit = scalarValues > MAX_MESSAGE_SCALAR_VALUES
  const content = overLimit ? null : buildMessageContent(text, mentions, workItems)
  const canSend = Boolean(content) && !disabled

  useLayoutEffect(() => {
    const element = textareaRef.current
    if (!element) return
    element.style.height = 'auto'
    element.style.height = `${Math.min(element.scrollHeight, 280)}px`
  }, [text])

  // An editor opens with the caret after the text.
  useEffect(() => {
    if (!autoFocus) return
    const element = textareaRef.current
    if (!element) return
    element.focus()
    element.setSelectionRange(element.value.length, element.value.length)
    // Only when it opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Search as the @ or # query changes, keeping only the latest answer.
  useEffect(() => {
    if (!trigger) return undefined
    const generation = ++searchGeneration.current
    const timer = window.setTimeout(() => {
      const search: Promise<Suggestion[]> =
        trigger.kind === 'mention'
          ? searchers.current.searchActors(trigger.query).then((actors) => actors.slice(0, 8).map((actor) => ({ kind: 'mention', actor })))
          : (searchers.current.searchWorkItems?.(trigger.query) ?? Promise.resolve([])).then((items) =>
              items.slice(0, 8).map((item) => ({ kind: 'work', item })),
            )
      search
        .then((next) => {
          if (searchGeneration.current !== generation) return
          setSuggestions(next)
          setActive(0)
        })
        .catch((reason: unknown) => {
          if (searchGeneration.current !== generation) return
          setSuggestions([])
          setSearchError(reason instanceof Error ? reason.message : 'Search didn’t work. Keep typing to try again.')
        })
        .finally(() => {
          if (searchGeneration.current === generation) setSearching(false)
        })
    }, 120)
    return () => window.clearTimeout(timer)
  }, [trigger])

  const updateTrigger = (value: string, caret: number) => {
    const next = activeTrigger(value, caret, Boolean(searchWorkItems))
    const same = trigger && next && trigger.kind === next.kind && trigger.query === next.query && trigger.start === next.start
    if (same) return
    setTrigger(next)
    if (next) {
      setSearching(true)
      setSearchError(null)
    } else {
      setSuggestions([])
    }
  }

  const closeSuggestions = () => {
    searchGeneration.current += 1
    setTrigger(null)
    setSuggestions([])
    setSearching(false)
  }

  const pick = (suggestion: Suggestion) => {
    if (!trigger) return
    const inserted = suggestion.kind === 'mention' ? `@${suggestion.actor.handle} ` : `${suggestion.item.key} `
    const caret = textareaRef.current?.selectionStart ?? text.length
    const next = text.slice(0, trigger.start) + inserted + text.slice(caret)
    setText(next)
    if (suggestion.kind === 'mention') {
      setMentions((current) =>
        current.some((actor) => actor.id === suggestion.actor.id) ? current : [...current, suggestion.actor],
      )
    } else {
      setWorkItems((current) =>
        current.some((item) => item.id === suggestion.item.id) ? current : [...current, suggestion.item],
      )
    }
    closeSuggestions()
    const position = trigger.start + inserted.length
    window.requestAnimationFrame(() => {
      textareaRef.current?.focus()
      textareaRef.current?.setSelectionRange(position, position)
    })
  }

  const insertTrigger = (character: '@' | '#') => {
    const element = textareaRef.current
    const caret = element?.selectionStart ?? text.length
    const needsSpace = caret > 0 && !/\s/.test(text[caret - 1])
    const inserted = `${needsSpace ? ' ' : ''}${character}`
    const next = text.slice(0, caret) + inserted + text.slice(caret)
    const position = caret + inserted.length
    setText(next)
    updateTrigger(next, position)
    window.requestAnimationFrame(() => {
      element?.focus()
      element?.setSelectionRange(position, position)
    })
  }

  const submit = (event?: FormEvent<HTMLFormElement>) => {
    event?.preventDefault()
    if (!content || !canSend || (keepDraftWhileSaving && busy)) return
    // Clear right away so it feels instant. A failed send puts the draft back and keeps its
    // idempotency key, so retrying can't post it twice; the next message gets a fresh key.
    const draft = { text, mentions, workItems }
    const key = idempotencyKeyRef.current
    idempotencyKeyRef.current = createIdempotencyKey()
    setBusy(true)
    setError(null)
    if (!keepDraftWhileSaving) {
      setText('')
      setMentions([])
      setWorkItems([])
    }
    void onSubmit({ content, idempotencyKey: key, text: draft.text.trim() })
      .catch((reason: unknown) => {
        idempotencyKeyRef.current = key
        setText((current) => (current.trim() ? current : draft.text))
        setMentions((current) => [...draft.mentions, ...current.filter((actor) => !draft.mentions.some((m) => m.id === actor.id))])
        setWorkItems((current) => [...draft.workItems, ...current.filter((item) => !draft.workItems.some((w) => w.id === item.id))])
        setError(reason instanceof Error ? reason.message : 'Your message wasn’t sent.')
      })
      .finally(() => {
        setBusy(false)
        queueMicrotask(() => textareaRef.current?.focus())
      })
  }

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (trigger && suggestions.length) {
      if (event.key === 'ArrowDown') {
        event.preventDefault()
        setActive((index) => (index + 1) % suggestions.length)
        return
      }
      if (event.key === 'ArrowUp') {
        event.preventDefault()
        setActive((index) => (index - 1 + suggestions.length) % suggestions.length)
        return
      }
      if (event.key === 'Enter' || event.key === 'Tab') {
        event.preventDefault()
        pick(suggestions[active] ?? suggestions[0])
        return
      }
    }
    if (trigger && event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      closeSuggestions()
      return
    }
    if (onCancel && event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      onCancel()
      return
    }
    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault()
      submit()
    }
  }

  if (disabledReason) {
    return (
      <div className="aw-composer-wrap">
        <div className="aw-composer is-disabled">
          <Icon name="lock" />
          <span>{disabledReason}</span>
        </div>
      </div>
    )
  }

  const listId = `${id}-suggestions`
  const remaining = MAX_MESSAGE_SCALAR_VALUES - scalarValues

  return (
    <form className={className ? `aw-composer-wrap ${className}` : 'aw-composer-wrap'} onSubmit={submit}>
      {toolbar ? <div className="aw-composer__toolbar">{toolbar}</div> : null}
      <div className={`aw-composer${busy ? ' is-busy' : ''}`}>
        {trigger ? (
          <div className="aw-mentions" id={listId} role="listbox" aria-label={trigger.kind === 'mention' ? 'People and agents' : 'Work items'}>
            <div className="aw-mentions__head">
              {trigger.kind === 'mention' ? 'People and agents' : 'Work items'}
              {trigger.query ? ` matching “${trigger.query}”` : ''}
            </div>
            {searchError ? <div className="aw-mentions__empty" role="alert">{searchError}</div> : null}
            {!searchError && !searching && suggestions.length === 0 ? (
              <div className="aw-mentions__empty">Nothing matches “{trigger.query}”.</div>
            ) : null}
            {suggestions.map((suggestion, index) => (
              <div
                aria-selected={index === active}
                className={index === active ? 'aw-mentions__item is-active' : 'aw-mentions__item'}
                id={`${listId}-${index}`}
                key={suggestion.kind === 'mention' ? suggestion.actor.id : suggestion.item.id}
                onMouseDown={(event) => {
                  event.preventDefault()
                  pick(suggestion)
                }}
                onMouseEnter={() => setActive(index)}
                role="option"
              >
                {suggestion.kind === 'mention' ? (
                  <>
                    <Avatar kind={actorKind(suggestion.actor.kind)} name={suggestion.actor.displayName} seed={suggestion.actor.id} size={24} />
                    <span className="aw-mentions__name">{suggestion.actor.displayName}</span>
                    <ActorTag kind={actorKind(suggestion.actor.kind)} />
                    <span className="aw-mentions__handle">@{suggestion.actor.handle}</span>
                  </>
                ) : (
                  <>
                    <Icon name="work" />
                    <span className="aw-mentions__key">{suggestion.item.key}</span>
                    <span className="aw-mentions__name aw-mentions__title">{suggestion.item.title}</span>
                  </>
                )}
              </div>
            ))}
          </div>
        ) : null}
        <label className="visually-hidden" htmlFor={id}>
          {label}
        </label>
        <textarea
          aria-activedescendant={trigger && suggestions.length ? `${listId}-${active}` : undefined}
          aria-autocomplete="list"
          aria-controls={trigger ? listId : undefined}
          aria-expanded={Boolean(trigger)}
          disabled={disabled}
          id={id}
          onBlur={() => window.setTimeout(closeSuggestions, 120)}
          onChange={(event) => {
            setText(event.target.value)
            updateTrigger(event.target.value, event.target.selectionStart ?? event.target.value.length)
          }}
          onKeyDown={onKeyDown}
          onSelect={(event) => {
            const element = event.currentTarget
            if (trigger) updateTrigger(element.value, element.selectionStart ?? element.value.length)
          }}
          placeholder={placeholder}
          ref={textareaRef}
          role="combobox"
          rows={1}
          value={text}
        />
        <div className="aw-composer__bar">
          <div className="aw-composer__tools">
            <IconButton disabled={disabled || busy} icon="at-sign" label="Mention someone" onClick={() => insertTrigger('@')} size="sm" />
            {searchWorkItems ? (
              <IconButton disabled={disabled || busy} icon="work" label="Reference a work item" onClick={() => insertTrigger('#')} size="sm" />
            ) : null}
          </div>
          <div className="aw-composer__end">
            {remaining < MAX_MESSAGE_SCALAR_VALUES * 0.1 ? (
              <span aria-live="polite" className={overLimit ? 'aw-composer__count is-over' : 'aw-composer__count'}>
                {remaining.toLocaleString()}
                <span className="visually-hidden"> characters left</span>
              </span>
            ) : null}
            {onCancel ? (
              <Button onClick={onCancel} size="sm" variant="ghost">
                Cancel
              </Button>
            ) : null}
            <button aria-label={submitLabel} className="aw-composer__send" disabled={!canSend || (keepDraftWhileSaving && busy)} type="submit">
              <Icon className={busy ? 'aw-spin' : undefined} name={busy ? 'loader' : submitIcon} />
            </button>
          </div>
        </div>
      </div>
      {overLimit ? (
        <p className="aw-composer__error" role="alert">
          Shorten this by {(scalarValues - MAX_MESSAGE_SCALAR_VALUES).toLocaleString()}{' '}
          {scalarValues - MAX_MESSAGE_SCALAR_VALUES === 1 ? 'character' : 'characters'} to send it.
        </p>
      ) : error ? (
        <p className="aw-composer__error" role="alert">
          <Icon name="circle-alert" size={14} />
          {error} {keepDraftWhileSaving ? 'Your text is still here.' : 'Your draft is still here. Press Enter to try again.'}
        </p>
      ) : note ? (
        <p className="aw-composer__note">{note}</p>
      ) : (
        <p aria-hidden={!text} className={text ? 'aw-composer__hint is-visible' : 'aw-composer__hint'}>
          <Kbd>Enter</Kbd> to send · <Kbd>Shift</Kbd> + <Kbd>Enter</Kbd> for a new line
        </p>
      )}
    </form>
  )
}

export { StructuredComposer as Composer }
