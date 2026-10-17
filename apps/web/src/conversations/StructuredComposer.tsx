import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ClipboardEvent,
  type DragEvent,
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
} from 'react'

import { isConnectionError, onReconnect, readLocal, writeLocal } from '../app/localDrafts'
import type { Uploader } from '../files/api'
import { UploadChips } from '../files/UploadChips'
import { useUploads } from '../files/useUploads'
import { ActorTag, Avatar, Button, Icon, IconButton, Kbd, actorKind, type IconGlyph, type IconName } from '../ui'
import { applyEdit, formatEdit, type FormatAction } from '../ui/markdown/editing'
import {
  activeTrigger,
  BLANK_MESSAGE_CONTENT,
  buildMessageContent,
  composerScalarValues,
  createIdempotencyKey,
  MAX_MESSAGE_SCALAR_VALUES,
  type ComposerDraft,
  type ComposerSubmission,
  type Trigger,
} from './composerModel'
import type { ConversationActor, ConversationWorkItemReference } from './types'
import { Bold, Code, Italic, Paperclip, SquareCode, TextQuote, Type } from '../files/icons'

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
  /**
   * Lets people attach files (picker, phone camera or library, paste, drop).
   * Each uploads at once; the submission carries the ones that finished.
   */
  upload?: Uploader
  /**
   * Keep the draft in this browser under this key (a conversation's id), so a
   * reload or a dropped connection doesn't lose it, and keep sends that fail
   * for want of a connection to send again when it's back.
   */
  draftKey?: string
}

/** A send that couldn't reach the server, kept to send again with the same key. */
type QueuedSend = ComposerSubmission & { text: string }

/** The formatting row, and Slack's shortcuts for it (Ctrl or ⌘). */
const FORMATS: { action: FormatAction; icon: IconGlyph; label: string; shortcut: string }[] = [
  { action: 'bold', icon: Bold, label: 'Bold', shortcut: 'B' },
  { action: 'italic', icon: Italic, label: 'Italic', shortcut: 'I' },
  { action: 'code', icon: Code, label: 'Code', shortcut: 'E' },
  { action: 'codeblock', icon: SquareCode, label: 'Code block', shortcut: 'Alt+Shift+C' },
  { action: 'bullet', icon: 'list', label: 'Bulleted list', shortcut: 'Shift+8' },
  { action: 'quote', icon: TextQuote, label: 'Quote', shortcut: 'Shift+9' },
]

function shortcutAction(event: KeyboardEvent<HTMLTextAreaElement>): FormatAction | null {
  if (!(event.metaKey || event.ctrlKey)) return null
  const key = event.key.toLowerCase()
  if (event.shiftKey && event.altKey && (key === 'c' || event.code === 'KeyC')) return 'codeblock'
  if (event.altKey) return null
  if (event.shiftKey) {
    if (event.code === 'Digit8' || key === '*') return 'bullet'
    if (event.code === 'Digit9' || key === '(') return 'quote'
    return null
  }
  if (key === 'b') return 'bold'
  if (key === 'i') return 'italic'
  if (key === 'e') return 'code'
  return null
}

type Suggestion =
  | { kind: 'mention'; actor: ConversationActor }
  | { kind: 'work'; item: ConversationWorkItemReference }

/** One rounded box: type, @mention people and agents inline, reference work items with #, Enter to send. */
export function StructuredComposer(props: StructuredComposerProps) {
  // A new conversation is a new draft.
  return <ComposerBox key={props.draftKey ?? ''} {...props} />
}

function ComposerBox({
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
  draftKey,
  upload,
}: StructuredComposerProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const uploads = useUploads(upload)
  const [dragging, setDragging] = useState(false)
  const [formatting, setFormatting] = useState(false)
  const idempotencyKeyRef = useRef(createIdempotencyKey())
  const searchGeneration = useRef(0)
  const id = useId()
  const [saved] = useState(() => (draftKey && !initialDraft ? readLocal<ComposerDraft>(`draft:${draftKey}`) : null))
  const [text, setText] = useState(initialDraft?.text ?? saved?.text ?? '')
  const [mentions, setMentions] = useState<ConversationActor[]>(initialDraft?.mentions ?? saved?.mentions ?? [])
  const [workItems, setWorkItems] = useState<ConversationWorkItemReference[]>(initialDraft?.workItems ?? saved?.workItems ?? [])
  const [queued, setQueued] = useState<QueuedSend[]>(() => (draftKey ? (readLocal<QueuedSend[]>(`unsent:${draftKey}`) ?? []) : []))
  const flushing = useRef(false)
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

  // The draft is saved as it's typed and forgotten once it's sent.
  useEffect(() => {
    if (!draftKey) return
    writeLocal<ComposerDraft>(`draft:${draftKey}`, text.trim() ? { text, mentions, workItems } : null)
  }, [draftKey, text, mentions, workItems])

  const queuedRef = useRef(queued)
  const queue = (update: (current: QueuedSend[]) => QueuedSend[]) => {
    const next = update(queuedRef.current)
    queuedRef.current = next
    if (draftKey) writeLocal<QueuedSend[]>(`unsent:${draftKey}`, next.length ? next : null)
    setQueued(next)
  }

  // Send what's waiting, oldest first, each with the key it was first sent
  // with, so a send that did reach the server isn't posted twice.
  const flush = async () => {
    if (flushing.current) return
    flushing.current = true
    try {
      for (const item of [...queuedRef.current]) {
        try {
          await onSubmit({ content: item.content, idempotencyKey: item.idempotencyKey, text: item.text, attachments: item.attachments })
          queue((current) => current.filter((entry) => entry.idempotencyKey !== item.idempotencyKey))
        } catch (reason) {
          if (isConnectionError(reason)) break
          // Refused for good: back into the box to change or copy.
          queue((current) => current.filter((entry) => entry.idempotencyKey !== item.idempotencyKey))
          setText((current) => (current.trim() ? `${item.text}\n${current}` : item.text))
          setError(reason instanceof Error ? reason.message : 'Your message wasn’t sent.')
        }
      }
    } finally {
      flushing.current = false
    }
  }
  const flushRef = useRef(flush)
  useEffect(() => {
    flushRef.current = flush
  })
  const hasQueued = queued.length > 0
  useEffect(() => {
    if (!hasQueued) return undefined
    void flushRef.current()
    return onReconnect(() => void flushRef.current())
  }, [hasQueued])

  const editQueued = (item: QueuedSend) => {
    queue((current) => current.filter((entry) => entry.idempotencyKey !== item.idempotencyKey))
    setText((current) => (current.trim() ? `${item.text}\n${current}` : item.text))
    window.requestAnimationFrame(() => textareaRef.current?.focus())
  }

  const scalarValues = composerScalarValues(text)
  const overLimit = scalarValues > MAX_MESSAGE_SCALAR_VALUES
  const typed = overLimit ? null : buildMessageContent(text, mentions, workItems)
  // A message can be only files.
  const content = typed ?? (!overLimit && uploads.ready.length > 0 ? BLANK_MESSAGE_CONTENT : null)
  const canSend = Boolean(content) && !disabled && !uploads.uploading

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

  const applyFormat = (action: FormatAction) => {
    const element = textareaRef.current
    if (!element) return
    const edit = formatEdit(action, { value: text, start: element.selectionStart, end: element.selectionEnd })
    setText(applyEdit(text, edit))
    window.requestAnimationFrame(() => {
      element.focus()
      element.setSelectionRange(edit.selectStart, edit.selectEnd)
    })
  }

  const attach = (files: Iterable<File>) => {
    const warning = uploads.add(files)
    setError(warning)
  }

  const onPaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    if (!upload) return
    const files = [...event.clipboardData.files]
    if (files.length === 0) return
    // A screenshot comes as a file alone; copied text that brings a picture along stays text.
    if (!event.clipboardData.types.includes('text/plain')) event.preventDefault()
    else return
    attach(files)
  }

  const hasFiles = (event: DragEvent) => Boolean(upload) && [...event.dataTransfer.types].includes('Files')
  const onDragOver = (event: DragEvent<HTMLFormElement>) => {
    if (!hasFiles(event) || disabled) return
    event.preventDefault()
    event.dataTransfer.dropEffect = 'copy'
    if (!dragging) setDragging(true)
  }
  const onDrop = (event: DragEvent<HTMLFormElement>) => {
    setDragging(false)
    if (!hasFiles(event) || disabled) return
    event.preventDefault()
    attach(event.dataTransfer.files)
  }

  const submit = (event?: FormEvent<HTMLFormElement>) => {
    event?.preventDefault()
    if (!content || !canSend || (keepDraftWhileSaving && busy)) return
    // Clear right away so it feels instant. A failed send puts the draft back and keeps its
    // idempotency key, so retrying can't post it twice; the next message gets a fresh key.
    const draft = { text, mentions, workItems }
    const attachments = uploads.ready.length ? uploads.ready : undefined
    const key = idempotencyKeyRef.current
    idempotencyKeyRef.current = createIdempotencyKey()
    setBusy(true)
    setError(null)
    if (!keepDraftWhileSaving) {
      setText('')
      setMentions([])
      setWorkItems([])
      uploads.clear()
    }
    void onSubmit({ content, idempotencyKey: key, text: draft.text.trim(), attachments })
      .catch((reason: unknown) => {
        if (draftKey && !keepDraftWhileSaving && isConnectionError(reason)) {
          // Kept and sent again when the connection is back; never dropped.
          queue((current) => [...current, { content, idempotencyKey: key, text: draft.text.trim(), attachments }])
          return
        }
        idempotencyKeyRef.current = key
        if (attachments && !keepDraftWhileSaving) uploads.restore(attachments)
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
    const format = shortcutAction(event)
    if (format) {
      event.preventDefault()
      applyFormat(format)
      return
    }
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
    <form
      className={className ? `aw-composer-wrap ${className}` : 'aw-composer-wrap'}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false)
      }}
      onDragOver={onDragOver}
      onDrop={onDrop}
      onSubmit={submit}
    >
      {queued.length ? (
        <div aria-label="Waiting to send" className="aw-composer__queued" role="list">
          {queued.map((item) => (
            <div className="aw-composer__queued-item" key={item.idempotencyKey} role="listitem">
              <Icon name="clock" size={14} />
              <span className="aw-composer__queued-text">
                {item.text || (item.attachments?.length ? `${item.attachments.length} ${item.attachments.length === 1 ? 'file' : 'files'}` : '')}
              </span>
              <Button onClick={() => editQueued(item)} size="sm" variant="ghost">
                Edit
              </Button>
            </div>
          ))}
          <p aria-live="polite" className="aw-composer__queued-note">
            {queued.length === 1 ? 'Not sent yet.' : `${queued.length} messages not sent yet.`} No connection; this sends by itself when you’re back online.{' '}
            <Button onClick={() => void flush()} size="sm" variant="ghost">
              Try now
            </Button>
          </p>
        </div>
      ) : null}
      {toolbar ? <div className="aw-composer__toolbar">{toolbar}</div> : null}
      <div className={`aw-composer${busy ? ' is-busy' : ''}${dragging ? ' is-dropping' : ''}`}>
        {dragging ? (
          <div aria-hidden="true" className="aw-composer__drop">
            <Icon name={Paperclip} />
            Drop to attach
          </div>
        ) : null}
        {formatting ? (
          <div aria-label="Formatting" className="aw-composer__formats" role="toolbar">
            {FORMATS.map((format) => (
              <IconButton
                disabled={disabled || busy}
                icon={format.icon}
                key={format.action}
                label={format.label}
                onClick={() => applyFormat(format.action)}
                onMouseDown={(event) => event.preventDefault()}
                shortcut={`Ctrl+${format.shortcut}`}
                size="sm"
              />
            ))}
          </div>
        ) : null}
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
          onPaste={onPaste}
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
        <UploadChips items={uploads.items} onRemove={uploads.remove} />
        <div className="aw-composer__bar">
          <div className="aw-composer__tools">
            {upload ? (
              <>
                <IconButton disabled={disabled || busy} icon={Paperclip} label="Attach files" onClick={() => fileInputRef.current?.click()} size="sm" />
                <input
                  className="visually-hidden"
                  multiple
                  onChange={(event) => {
                    if (event.target.files) attach(event.target.files)
                    event.target.value = ''
                  }}
                  ref={fileInputRef}
                  tabIndex={-1}
                  type="file"
                />
              </>
            ) : null}
            <IconButton disabled={disabled || busy} icon="at-sign" label="Mention someone" onClick={() => insertTrigger('@')} size="sm" />
            {searchWorkItems ? (
              <IconButton disabled={disabled || busy} icon="work" label="Reference a work item" onClick={() => insertTrigger('#')} size="sm" />
            ) : null}
            <IconButton
              disabled={disabled}
              icon={Type}
              label={formatting ? 'Hide formatting' : 'Formatting'}
              onClick={() => setFormatting((shown) => !shown)}
              pressed={formatting}
              size="sm"
            />
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
