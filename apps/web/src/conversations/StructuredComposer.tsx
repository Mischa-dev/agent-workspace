import { FormEvent, useRef, useState } from 'react'

import { MentionPicker } from './MentionPicker'
import {
  buildMessageContent,
  composerScalarValues,
  createIdempotencyKey,
  MAX_MESSAGE_SCALAR_VALUES,
  type ComposerSubmission,
} from './composerModel'
import type { ConversationActor } from './types'

type StructuredComposerProps = {
  disabled?: boolean
  onSubmit: (submission: ComposerSubmission) => Promise<void>
  searchActors: (query: string) => Promise<ConversationActor[]>
}

export function StructuredComposer({
  disabled = false,
  onSubmit,
  searchActors,
}: StructuredComposerProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const mentionButtonRef = useRef<HTMLButtonElement>(null)
  const idempotencyKeyRef = useRef(createIdempotencyKey())
  const searchGenerationRef = useRef(0)
  const [text, setText] = useState('')
  const [mentions, setMentions] = useState<ConversationActor[]>([])
  const [pickerOpen, setPickerOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<ConversationActor[]>([])
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

  const openMentionPicker = () => {
    setPickerOpen(true)
    runMentionSearch(query)
  }

  const closeMentionPicker = () => {
    searchGenerationRef.current += 1
    setPickerOpen(false)
  }

  const changeMentionQuery = (nextQuery: string) => {
    setQuery(nextQuery)
    runMentionSearch(nextQuery)
  }

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const content = buildMessageContent(text, mentions)
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

  const scalarValues = composerScalarValues(text, mentions)
  const overLimit = scalarValues > MAX_MESSAGE_SCALAR_VALUES
  const content = overLimit ? null : buildMessageContent(text, mentions)
  return (
    <form className="structured-composer" onSubmit={submit}>
      <label htmlFor="conversation-message">Message</label>
      <textarea
        disabled={disabled || submitBusy}
        id="conversation-message"
        onChange={(event) => setText(event.target.value)}
        placeholder="Write a Message…"
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
        <span
          aria-live="polite"
          className={overLimit ? 'character-count over-limit' : 'character-count'}
        >
          {scalarValues.toLocaleString()} / 20,000 Unicode characters
        </span>
        <button disabled={!content || disabled || submitBusy} type="submit">
          {submitBusy ? 'Sending…' : 'Send Message'}
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
    </form>
  )
}
