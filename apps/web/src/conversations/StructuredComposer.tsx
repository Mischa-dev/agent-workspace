import { FormEvent, useRef, useState } from 'react'

import { MentionPicker } from './MentionPicker'
import { WorkItemPicker } from './WorkItemPicker'
import {
  buildMessageContent,
  composerScalarValues,
  createIdempotencyKey,
  MAX_MESSAGE_SCALAR_VALUES,
  type ComposerSubmission,
} from './composerModel'
import type { ConversationActor, ConversationWorkItemReference } from './types'

type StructuredComposerProps = {
  disabled?: boolean
  label?: string
  onSubmit: (submission: ComposerSubmission) => Promise<void>
  placeholder?: string
  searchActors: (query: string) => Promise<ConversationActor[]>
  searchWorkItems?: (query: string) => Promise<ConversationWorkItemReference[]>
  submitLabel?: string
}

export function StructuredComposer({
  disabled = false,
  label = 'Message',
  onSubmit,
  placeholder = 'Write a Message…',
  searchActors,
  searchWorkItems,
  submitLabel = 'Send Message',
}: StructuredComposerProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const mentionButtonRef = useRef<HTMLButtonElement>(null)
  const workItemButtonRef = useRef<HTMLButtonElement>(null)
  const idempotencyKeyRef = useRef(createIdempotencyKey())
  const searchGenerationRef = useRef(0)
  const [text, setText] = useState('')
  const [mentions, setMentions] = useState<ConversationActor[]>([])
  const [workItems, setWorkItems] = useState<ConversationWorkItemReference[]>([])
  const [pickerOpen, setPickerOpen] = useState(false)
  const [workItemPickerOpen, setWorkItemPickerOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<ConversationActor[]>([])
  const [workItemResults, setWorkItemResults] = useState<ConversationWorkItemReference[]>([])
  const [searchBusy, setSearchBusy] = useState(false)
  const [searchError, setSearchError] = useState<string | null>(null)
  const [submitBusy, setSubmitBusy] = useState(false)
  const [submitError, setSubmitError] = useState<string | null>(null)

  const runMentionSearch = (nextQuery: string) => {
    const generation = ++searchGenerationRef.current
    setSearchBusy(true)
    setSearchError(null)
    void searchActors(nextQuery.trim())
      .then((actors) => {
        if (searchGenerationRef.current === generation) setResults(actors)
      })
      .catch((reason: unknown) => {
        if (searchGenerationRef.current !== generation) return
        setSearchError(
          reason instanceof Error
            ? reason.message
            : 'Could not search Project members.',
        )
      })
      .finally(() => {
        if (searchGenerationRef.current === generation) setSearchBusy(false)
      })
  }

  const runWorkItemSearch = (nextQuery: string) => {
    if (!searchWorkItems) return
    const generation = ++searchGenerationRef.current
    setSearchBusy(true)
    setSearchError(null)
    void searchWorkItems(nextQuery.trim())
      .then((items) => { if (searchGenerationRef.current === generation) setWorkItemResults(items) })
      .catch((reason: unknown) => {
        if (searchGenerationRef.current === generation) setSearchError(reason instanceof Error ? reason.message : 'Could not search Work Items.')
      })
      .finally(() => { if (searchGenerationRef.current === generation) setSearchBusy(false) })
  }

  const openMentionPicker = () => {
    setWorkItemPickerOpen(false)
    setPickerOpen(true)
    runMentionSearch(query)
  }

  const openWorkItemPicker = () => {
    setPickerOpen(false)
    setWorkItemPickerOpen(true)
    runWorkItemSearch(query)
  }

  const closeMentionPicker = () => {
    searchGenerationRef.current += 1
    setPickerOpen(false)
  }

  const closeWorkItemPicker = () => {
    searchGenerationRef.current += 1
    setWorkItemPickerOpen(false)
  }

  const changeMentionQuery = (nextQuery: string) => {
    setQuery(nextQuery)
    runMentionSearch(nextQuery)
  }

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const content = buildMessageContent(text, mentions, workItems)
    if (
      !content ||
      composerScalarValues(text, mentions) > MAX_MESSAGE_SCALAR_VALUES ||
      disabled ||
      submitBusy
    ) {
      return
    }

    setSubmitBusy(true)
    setSubmitError(null)
    void onSubmit({
      content,
      idempotencyKey: idempotencyKeyRef.current,
    })
      .then(() => {
        setText('')
        setMentions([])
        setWorkItems([])
        idempotencyKeyRef.current = createIdempotencyKey()
        queueMicrotask(() => textareaRef.current?.focus())
      })
      .catch((reason: unknown) => {
        setSubmitError(
          reason instanceof Error ? reason.message : 'Message could not be sent.',
        )
      })
      .finally(() => setSubmitBusy(false))
  }

  const addMention = (actor: ConversationActor) => {
    setMentions((current) =>
      current.some((candidate) => candidate.id === actor.id)
        ? current
        : [...current, actor].slice(0, 32),
    )
    setQuery('')
  }

  const addWorkItem = (item: ConversationWorkItemReference) => {
    setWorkItems((current) => current.some((candidate) => candidate.id === item.id) ? current : [...current, item].slice(0, 32))
    setQuery('')
  }

  const scalarValues = composerScalarValues(text, mentions, workItems)
  const overLimit = scalarValues > MAX_MESSAGE_SCALAR_VALUES
  const content = overLimit ? null : buildMessageContent(text, mentions, workItems)
  return (
    <form className="structured-composer" onSubmit={submit}>
      <label htmlFor="conversation-message">{label}</label>
      <textarea
        disabled={disabled || submitBusy}
        id="conversation-message"
        onChange={(event) => setText(event.target.value)}
        placeholder={placeholder}
        ref={textareaRef}
        rows={3}
        value={text}
      />
      {mentions.length > 0 ? (
        <ul aria-label="Selected mentions" className="selected-mentions">
          {mentions.map((actor) => (
            <li key={actor.id}>
              <span>@{actor.handle}</span>
              <button
                aria-label={`Remove @${actor.handle}`}
                disabled={submitBusy}
                onClick={() =>
                  setMentions((current) =>
                    current.filter((candidate) => candidate.id !== actor.id),
                  )
                }
                type="button"
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      ) : null}
      {workItems.length > 0 ? (
        <ul aria-label="Selected Work Item references" className="selected-mentions">
          {workItems.map((item) => (
            <li key={item.id}><span>{item.key}</span><button aria-label={`Remove ${item.key}`} disabled={submitBusy} onClick={() => setWorkItems((current) => current.filter((candidate) => candidate.id !== item.id))} type="button">×</button></li>
          ))}
        </ul>
      ) : null}
      <div className="composer-actions">
        <button
          aria-expanded={pickerOpen}
          disabled={disabled || submitBusy || mentions.length >= 32}
          onClick={openMentionPicker}
          ref={mentionButtonRef}
          type="button"
        >
          Mention Actor
        </button>
        {searchWorkItems ? <button aria-expanded={workItemPickerOpen} disabled={disabled || submitBusy || workItems.length >= 32} onClick={openWorkItemPicker} ref={workItemButtonRef} type="button">Reference Work Item</button> : null}
        <span
          aria-live="polite"
          className={overLimit ? 'character-count over-limit' : 'character-count'}
        >
          {scalarValues.toLocaleString()} / 20,000 Unicode characters
        </span>
        <button disabled={!content || disabled || submitBusy} type="submit">
          {submitBusy ? 'Sending…' : submitLabel}
        </button>
      </div>
      {overLimit ? (
        <p role="alert">
          Shorten this Message by{' '}
          {(scalarValues - MAX_MESSAGE_SCALAR_VALUES).toLocaleString()} Unicode
          {scalarValues - MAX_MESSAGE_SCALAR_VALUES === 1
            ? ' character'
            : ' characters'}.
        </p>
      ) : null}
      {submitError ? <p role="alert">{submitError}</p> : null}
      {pickerOpen ? (
        <MentionPicker
          actors={results}
          busy={searchBusy}
          error={searchError}
          onClose={closeMentionPicker}
          onQueryChange={changeMentionQuery}
          onSelect={addMention}
          returnFocusRef={mentionButtonRef}
        />
      ) : null}
      {workItemPickerOpen ? (
        <WorkItemPicker busy={searchBusy} error={searchError} items={workItemResults} onClose={closeWorkItemPicker} onQueryChange={(nextQuery) => { setQuery(nextQuery); runWorkItemSearch(nextQuery) }} onSelect={addWorkItem} returnFocusRef={workItemButtonRef} />
      ) : null}
    </form>
  )
}
