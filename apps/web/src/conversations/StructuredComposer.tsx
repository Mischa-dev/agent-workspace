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

import { holdUnsent, isConnectionError, onReconnect, readLocal, writeLocal } from '../app/localDrafts'
import type { Uploader } from '../files/api'
import { UploadChips } from '../files/UploadChips'
import { useUploads } from '../files/useUploads'
import { matchSlashCommand, suggestSlashCommands, type SlashCommand, type SlashOutcome } from '../later/slashCommands'
import { TimeDialog } from '../later/TimeDialog'
import { whenChoices } from '../later/when'
import {
  ActorTag,
  Avatar,
  Button,
  Icon,
  IconButton,
  Kbd,
  Menu,
  actorKind,
  countLabel,
  describeError,
  formatShortcut,
  type IconGlyph,
  type IconName,
} from '../ui'
import { applyEdit, formatEdit, type FormatAction } from '../ui/markdown/editing'
import {
  activeTrigger,
  BLANK_MESSAGE_CONTENT,
  buildMessageContent,
  composerScalarValues,
  createIdempotencyKey,
  MAX_MESSAGE_SCALAR_VALUES,
  renameHandles,
  typedHandles,
  type ComposerDraft,
  type ComposerSubmission,
  type OutboxTarget,
  type QueuedSend,
  type Trigger,
} from './composerModel'
import type { ConversationActor, ConversationWorkItemReference, MessageContentV1 } from './types'
import { Bold, Code, Italic, Paperclip, SquareCode, TextQuote, Type } from '../files/icons'
import { useToastOffset } from './toastOffset'

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
  /** Where a send kept for want of a connection goes, so it goes out from any page once the connection is back. */
  outbox?: OutboxTarget
  /** Slash commands offered as you type "/"; text naming none is sent as typed. */
  commands?: SlashCommand[]
  /** Run a command; `build` turns text into content with the people and tasks picked here. */
  onCommand?: (
    command: SlashCommand,
    args: string,
    build: (text: string) => MessageContentV1 | null,
  ) => Promise<SlashOutcome>
  /** Shows Schedule next to Send: send this message at a later time. */
  onSchedule?: (content: MessageContentV1, at: Date) => Promise<void>
  /** Suggest only people after @, for a project whose layout leaves agent mentions out. Typed handles still work. */
  peopleOnly?: boolean
  /** Up arrow in an empty box: edit your newest message. True when there was one. */
  onEditLast?: () => boolean
  /**
   * Put the cursor here when the conversation opens, so typing goes into the
   * message rather than setting off shortcuts. Only with a mouse or trackpad:
   * on a touch screen it would pop the keyboard up.
   */
  focusOnOpen?: boolean
}


/** The formatting row, and Slack's shortcuts for it (Ctrl, or ⌘ on a Mac). */
const FORMATS: { action: FormatAction; icon: IconGlyph; label: string; shortcut: string }[] = [
  { action: 'bold', icon: Bold, label: 'Bold', shortcut: 'mod+B' },
  { action: 'italic', icon: Italic, label: 'Italic', shortcut: 'mod+I' },
  { action: 'code', icon: Code, label: 'Code', shortcut: 'mod+E' },
  { action: 'codeblock', icon: SquareCode, label: 'Code block', shortcut: 'mod+alt+shift+C' },
  { action: 'bullet', icon: 'list', label: 'Bulleted list', shortcut: 'mod+shift+8' },
  { action: 'quote', icon: TextQuote, label: 'Quote', shortcut: 'mod+shift+9' },
]

/** A touch screen, where Enter makes a new line and the button sends. */
function coarsePointer() {
  return typeof window !== 'undefined' && typeof window.matchMedia === 'function' && window.matchMedia('(pointer: coarse)').matches
}

/** A reason as the end of a sentence. */
function sentence(text: string) {
  const trimmed = text.trim()
  return /[.!?…]["”’)]*$/.test(trimmed) ? trimmed : `${trimmed}.`
}

/** What a refused send says: the server's own words when it gave some. */
function sendFailure(reason: unknown) {
  return reason instanceof Error && reason.message ? reason.message : describeError(reason).message
}

/**
 * The line under the box. A failed send says how to send again; a note (a
 * file that can't be attached, a command that didn't run) says only itself.
 */
type ComposerError = { kind: 'send' | 'note'; text: string }

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

/** Outside the component: the time is read when it's picked. */
function isPast(at: Date) {
  return at.getTime() <= Date.now()
}

type Suggestion =
  | { kind: 'mention'; actor: ConversationActor }
  | { kind: 'work'; item: ConversationWorkItemReference }

/** One rounded box: type, @mention people and agents inline, reference tasks with #, Enter to send. */
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
  outbox,
  upload,
  commands,
  onCommand,
  onSchedule,
  peopleOnly = false,
  onEditLast,
  focusOnOpen = false,
}: StructuredComposerProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const wrapRef = useRef<HTMLDivElement & HTMLFormElement>(null)
  // A conversation's own composer (it keeps a draft) lifts the toasts above it.
  useToastOffset(wrapRef, Boolean(draftKey), Boolean(disabledReason))
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
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  const [trigger, setTrigger] = useState<Trigger | null>(null)
  const [suggestions, setSuggestions] = useState<Suggestion[]>([])
  const [active, setActive] = useState(0)
  const [searching, setSearching] = useState(false)
  const [searchError, setSearchError] = useState<string | null>(null)
  const [sending, setBusy] = useState(false)
  const [scheduling, setScheduling] = useState(false)
  const busy = sending || scheduling
  const [error, setError] = useState<ComposerError | null>(null)
  const showNote = (text: string | null) => setError(text ? { kind: 'note', text } : null)
  const [resolving, setResolving] = useState(false)
  // Text whose unknown @handles were pointed out once: the next send goes as typed.
  const [plainText, setPlainText] = useState<string | null>(null)
  const textRef = useRef('')
  const [slashActive, setSlashActive] = useState(0)
  const [slashClosed, setSlashClosed] = useState(false)
  const [customSchedule, setCustomSchedule] = useState(false)
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

  // While it's on screen, this box sends its own queue; the app sends it after.
  useEffect(() => (draftKey && !keepDraftWhileSaving ? holdUnsent(draftKey) : undefined), [draftKey, keepDraftWhileSaving])

  const queuedRef = useRef(queued)
  const queue = (update: (current: QueuedSend[]) => QueuedSend[]) => {
    const next = update(queuedRef.current)
    queuedRef.current = next
    if (draftKey) writeLocal<QueuedSend[]>(`unsent:${draftKey}`, next.length ? next : null)
    setQueued(next)
  }

  // Back into the box: the text above what's there, with its people, tasks and files.
  const restoreQueued = (item: QueuedSend) => {
    queue((current) => current.filter((entry) => entry.idempotencyKey !== item.idempotencyKey))
    setText((current) => (current.trim() ? `${item.text}\n${current}` : item.text))
    const mentioned = item.mentions ?? []
    const referenced = item.workItems ?? []
    setMentions((current) => [...mentioned, ...current.filter((actor) => !mentioned.some((m) => m.id === actor.id))])
    setWorkItems((current) => [...referenced, ...current.filter((work) => !referenced.some((w) => w.id === work.id))])
    if (item.attachments?.length) uploads.restore(item.attachments)
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
          restoreQueued(item)
          setError({ kind: 'send', text: sendFailure(reason) })
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
    restoreQueued(item)
    window.requestAnimationFrame(() => textareaRef.current?.focus())
  }

  const scalarValues = composerScalarValues(text)
  const overLimit = scalarValues > MAX_MESSAGE_SCALAR_VALUES
  const typed = overLimit ? null : buildMessageContent(text, mentions, workItems)
  // A message can be only files.
  const content = typed ?? (!overLimit && uploads.ready.length > 0 ? BLANK_MESSAGE_CONTENT : null)
  const canSend = Boolean(content) && !disabled && !uploads.uploading && !resolving
  useEffect(() => {
    textRef.current = text
  })
  const slashMatches = commands && onCommand && !trigger && !slashClosed ? suggestSlashCommands(text, commands) : []

  useLayoutEffect(() => {
    const element = textareaRef.current
    if (!element) return
    element.style.height = 'auto'
    element.style.height = `${Math.min(element.scrollHeight, 280)}px`
  }, [text])

  // An editor opens with the caret after the text; a conversation, with the
  // cursor in its box (with a mouse or trackpad).
  useEffect(() => {
    if (!autoFocus && !(focusOnOpen && !coarsePointer())) return
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
          ? searchers.current
              .searchActors(trigger.query)
              .then((actors) =>
                actors
                  .filter((actor) => !peopleOnly || actor.kind === 'human')
                  .slice(0, 8)
                  .map((actor) => ({ kind: 'mention', actor })),
              )
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
          setSearchError(`Results didn’t load. ${sentence(describeError(reason).message)} Keep typing to try again.`)
        })
        .finally(() => {
          if (searchGeneration.current === generation) setSearching(false)
        })
    }, 120)
    return () => window.clearTimeout(timer)
  }, [peopleOnly, trigger])

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
    showNote(uploads.add(files))
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

  const clearDraft = () => {
    setText('')
    setMentions([])
    setWorkItems([])
  }

  const completeSlash = (command: SlashCommand) => {
    const next = `/${command.name} `
    setText(next)
    setSlashClosed(true)
    window.requestAnimationFrame(() => {
      textareaRef.current?.focus()
      textareaRef.current?.setSelectionRange(next.length, next.length)
    })
  }

  // A command runs instead of sending; its text stays in the box if it fails.
  const runCommand = (command: SlashCommand, args: string) => {
    if (!onCommand) return
    setBusy(true)
    setError(null)
    void onCommand(command, args, (value) => buildMessageContent(value, mentions, workItems))
      .then((outcome) => {
        if (outcome.kind === 'replace') {
          setText(outcome.text)
          updateTrigger(outcome.text, outcome.text.length)
          window.requestAnimationFrame(() => {
            textareaRef.current?.focus()
            textareaRef.current?.setSelectionRange(outcome.text.length, outcome.text.length)
          })
        } else {
          clearDraft()
        }
      })
      .catch((reason: unknown) => showNote(`That command didn’t run. ${sentence(describeError(reason).message)}`))
      .finally(() => setBusy(false))
  }

  const schedule = async (at: Date) => {
    if (!content || !onSchedule || scheduling) return
    if (isPast(at)) throw new Error('That time has passed.')
    // Busy while it schedules, so Enter can't send the same text as well.
    setScheduling(true)
    setError(null)
    try {
      await onSchedule(content, at)
      clearDraft()
    } finally {
      setScheduling(false)
    }
  }

  // Send this draft. The box clears at once so it feels instant; a failed send
  // puts the draft back and keeps its idempotency key, so sending again can't
  // post it twice. The next message gets a fresh key.
  const send = (draft: ComposerDraft) => {
    const built = buildMessageContent(draft.text, draft.mentions, draft.workItems)
    const message = built ?? (uploads.ready.length > 0 ? BLANK_MESSAGE_CONTENT : null)
    if (!message) return
    const attachments = uploads.ready.length ? uploads.ready : undefined
    const key = idempotencyKeyRef.current
    idempotencyKeyRef.current = createIdempotencyKey()
    setBusy(true)
    setError(null)
    setPlainText(null)
    if (keepDraftWhileSaving) {
      setText(draft.text)
      setMentions(draft.mentions)
    } else {
      setText('')
      setMentions([])
      setWorkItems([])
      uploads.clear()
    }
    const sentText = draft.text.trim()
    void onSubmit({ content: message, idempotencyKey: key, text: sentText, attachments, mentions: draft.mentions, workItems: draft.workItems })
      .catch((reason: unknown) => {
        if (draftKey && !keepDraftWhileSaving && isConnectionError(reason)) {
          // Kept and sent again when the connection is back; never dropped.
          queue((current) => [
            ...current,
            { content: message, idempotencyKey: key, text: sentText, attachments, mentions: draft.mentions, workItems: draft.workItems, target: outbox },
          ])
          return
        }
        if (!mounted.current) {
          // The box is gone (they moved on while it sent): keep the text as
          // this conversation's draft so it's there when they come back.
          if (draftKey && !keepDraftWhileSaving && sentText) {
            const left = readLocal<ComposerDraft>(`draft:${draftKey}`)
            writeLocal<ComposerDraft>(`draft:${draftKey}`, {
              text: left?.text.trim() ? `${draft.text}\n${left.text}` : draft.text,
              mentions: [...draft.mentions, ...(left?.mentions ?? []).filter((actor) => !draft.mentions.some((m) => m.id === actor.id))],
              workItems: [...draft.workItems, ...(left?.workItems ?? []).filter((item) => !draft.workItems.some((w) => w.id === item.id))],
            })
          }
          return
        }
        idempotencyKeyRef.current = key
        if (attachments && !keepDraftWhileSaving) uploads.restore(attachments)
        setText((current) => (current.trim() ? current : draft.text))
        setMentions((current) => [...draft.mentions, ...current.filter((actor) => !draft.mentions.some((m) => m.id === actor.id))])
        setWorkItems((current) => [...draft.workItems, ...current.filter((item) => !draft.workItems.some((w) => w.id === item.id))])
        setError({ kind: 'send', text: sendFailure(reason) })
      })
      .finally(() => {
        setBusy(false)
        queueMicrotask(() => textareaRef.current?.focus())
      })
  }

  // "@muse can you look", typed without stopping at the list, still asks
  // Muse: each hand-typed handle is looked up, and one that matches someone
  // exactly becomes a mention. One that matches nobody is pointed out before
  // sending, and sending again sends it as plain text.
  const resolveAndSend = async (draft: ComposerDraft, handles: string[]) => {
    setResolving(true)
    setError(null)
    const looked = await Promise.all(
      handles.map((handle) =>
        searchers.current
          .searchActors(handle)
          .then((actors) => ({ handle, actor: actors.find((actor) => actor.handle.toLowerCase() === handle.toLowerCase()) ?? null, failed: false }))
          .catch(() => ({ handle, actor: null, failed: true })),
      ),
    )
    if (!mounted.current) return
    setResolving(false)
    // They kept typing: the next Enter looks again.
    if (textRef.current !== draft.text) return
    const found = looked.flatMap((entry) => (entry.actor ? [entry.actor] : []))
    const canonical = new Map(found.map((actor) => [actor.handle.toLowerCase(), actor.handle]))
    const next: ComposerDraft = {
      text: canonical.size ? renameHandles(draft.text, canonical) : draft.text,
      mentions: [...draft.mentions, ...found.filter((actor) => !draft.mentions.some((m) => m.id === actor.id))],
      workItems: draft.workItems,
    }
    // A lookup that failed can't say who's there, so that handle goes as typed.
    const unknown = looked.filter((entry) => !entry.actor && !entry.failed).map((entry) => `@${entry.handle}`)
    if (!unknown.length) {
      send(next)
      return
    }
    setText(next.text)
    setMentions(next.mentions)
    setPlainText(next.text)
    const names = unknown.length === 1 ? unknown[0] : `${unknown.slice(0, -1).join(', ')} and ${unknown[unknown.length - 1]}`
    const again = coarsePointer() ? 'Tap send again' : 'Press Enter again'
    showNote(`${names} ${unknown.length === 1 ? 'isn’t' : 'aren’t'} anyone here. ${again} to send it as plain text.`)
    window.requestAnimationFrame(() => textareaRef.current?.focus())
  }

  const submit = (event?: FormEvent<HTMLFormElement>) => {
    event?.preventDefault()
    const command = onCommand && commands ? matchSlashCommand(text, commands) : null
    if (command && !busy && !disabled) {
      runCommand(command.command, command.args)
      return
    }
    if (!content || !canSend || scheduling || (keepDraftWhileSaving && busy)) return
    // A picked mention typed over in other letters is still that mention.
    const picked = new Map(mentions.map((actor) => [actor.handle.toLowerCase(), actor.handle]))
    const draft = { text: picked.size ? renameHandles(text, picked) : text, mentions, workItems }
    const handles = plainText === text ? [] : typedHandles(text, mentions)
    if (handles.length) {
      void resolveAndSend(draft, handles)
      return
    }
    send(draft)
  }

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    const format = shortcutAction(event)
    if (format) {
      event.preventDefault()
      applyFormat(format)
      return
    }
    if (slashMatches.length) {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault()
        const step = event.key === 'ArrowDown' ? 1 : -1
        setSlashActive((index) => (index + step + slashMatches.length) % slashMatches.length)
        return
      }
      if ((event.key === 'Enter' && !event.shiftKey) || event.key === 'Tab') {
        event.preventDefault()
        completeSlash(slashMatches[slashActive] ?? slashMatches[0])
        return
      }
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopPropagation()
        setSlashClosed(true)
        return
      }
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
    if (event.key === 'ArrowUp' && onEditLast && !text && !event.shiftKey && !event.altKey && !event.metaKey && !event.ctrlKey) {
      if (onEditLast()) event.preventDefault()
      return
    }
    // On a touch screen Enter makes a new line, as in Slack's apps; the button sends.
    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing && !coarsePointer()) {
      event.preventDefault()
      submit()
    }
  }

  if (disabledReason) {
    return (
      <div className="aw-composer-wrap" ref={wrapRef}>
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
    <>
    <form
      className={className ? `aw-composer-wrap ${className}` : 'aw-composer-wrap'}
      data-composer={draftKey ? 'conversation' : undefined}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false)
      }}
      onDragOver={onDragOver}
      onDrop={onDrop}
      onSubmit={submit}
      ref={wrapRef}
    >
      {queued.length ? (
        <div aria-label="Waiting to send" className="aw-composer__queued" role="list">
          {queued.map((item) => (
            <div className="aw-composer__queued-item" key={item.idempotencyKey} role="listitem">
              <Icon name="clock" size={14} />
              <span className="aw-composer__queued-text">
                {item.text || (item.attachments?.length ? countLabel(item.attachments.length, 'file') : '')}
              </span>
              <Button onClick={() => editQueued(item)} size="sm" variant="ghost">
                Edit
              </Button>
            </div>
          ))}
          <p aria-live="polite" className="aw-composer__queued-note">
            {queued.length === 1 ? 'Not sent yet.' : `${countLabel(queued.length, 'message')} not sent yet.`} No connection. {queued.length === 1 ? 'It sends' : 'They send'} when you’re back online with this conversation open.{' '}
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
                shortcut={formatShortcut(format.shortcut)}
                size="sm"
              />
            ))}
          </div>
        ) : null}
        {slashMatches.length ? (
          <div aria-label="Commands" className="aw-mentions" id={listId} role="listbox">
            <div className="aw-mentions__head">Commands</div>
            {slashMatches.map((command, index) => (
              <div
                aria-selected={index === slashActive}
                className={index === slashActive ? 'aw-mentions__item is-active' : 'aw-mentions__item'}
                id={`${listId}-${index}`}
                key={command.name}
                onMouseDown={(event) => {
                  event.preventDefault()
                  completeSlash(command)
                }}
                onMouseEnter={() => setSlashActive(index)}
                role="option"
              >
                <Icon name={command.icon} />
                <span className="aw-mentions__key">/{command.name}</span>
                <span className="aw-mentions__name aw-mentions__title">{command.description}</span>
              </div>
            ))}
          </div>
        ) : null}
        {trigger ? (
          <div className="aw-mentions" id={listId} role="listbox" aria-label={trigger.kind === 'mention' ? 'People and agents' : 'Tasks'}>
            <div className="aw-mentions__head">
              {trigger.kind === 'mention' ? 'People and agents' : 'Tasks'}
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
          aria-activedescendant={
            slashMatches.length ? `${listId}-${slashActive}` : trigger && suggestions.length ? `${listId}-${active}` : undefined
          }
          aria-autocomplete="list"
          aria-controls={trigger || slashMatches.length ? listId : undefined}
          aria-expanded={Boolean(trigger) || slashMatches.length > 0}
          disabled={disabled}
          id={id}
          onBlur={() => window.setTimeout(closeSuggestions, 120)}
          onChange={(event) => {
            setText(event.target.value)
            setSlashClosed(false)
            setSlashActive(0)
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
                  aria-label="Choose files to attach"
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
              <IconButton disabled={disabled || busy} icon="work" label="Reference a task" onClick={() => insertTrigger('#')} size="sm" />
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
            {onSchedule ? (
              <Menu
                items={[
                  { heading: 'Send later' },
                  ...whenChoices().map((choice) => ({
                    label: choice.label,
                    icon: 'clock' as const,
                    // The time is worked out when it's picked, not when the menu was drawn.
                    onSelect: () =>
                      void schedule(whenChoices().find((now) => now.id === choice.id)?.at ?? choice.at).catch((reason: unknown) =>
                        showNote(`Not scheduled. ${sentence(describeError(reason).message)}`),
                      ),
                  })),
                  { label: 'Custom time…', icon: 'clock' as const, onSelect: () => setCustomSchedule(true) },
                ]}
                label="Schedule message"
                placement="bottom-end"
                trigger={
                  <IconButton
                    disabled={!canSend || busy || uploads.items.length > 0 || Boolean(matchSlashCommand(text, commands))}
                    icon="clock"
                    label={uploads.items.length > 0 ? 'Files can’t be scheduled yet' : 'Schedule message'}
                    size="sm"
                  />
                }
              />
            ) : null}
            <button aria-label={submitLabel} className="aw-composer__send" disabled={!canSend || scheduling || (keepDraftWhileSaving && busy)} type="submit">
              <Icon className={busy || resolving ? 'aw-spin' : undefined} name={busy || resolving ? 'loader' : submitIcon} />
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
          {error.kind !== 'send'
            ? error.text
            : keepDraftWhileSaving
              ? // An edit's reasons say what to do next themselves.
                `Not saved. ${sentence(error.text)}`
              : `Not sent. ${sentence(error.text)} ${coarsePointer() ? 'Tap send to try again.' : 'Press Enter to try again.'}`}
        </p>
      ) : note ? (
        <p className="aw-composer__note">{note}</p>
      ) : text.startsWith('/') && commands && matchSlashCommand(text, commands) ? (
        <p className="aw-composer__hint is-visible">
          <Kbd>{formatShortcut('enter')}</Kbd> to run /{matchSlashCommand(text, commands)?.command.name} · {matchSlashCommand(text, commands)?.command.usage || matchSlashCommand(text, commands)?.command.description}
        </p>
      ) : coarsePointer() ? null : (
        <p aria-hidden={!text} className={text ? 'aw-composer__hint is-visible' : 'aw-composer__hint'}>
          <Kbd>{formatShortcut('enter')}</Kbd> to send · <Kbd>{formatShortcut('shift+enter')}</Kbd> for a new line
        </p>
      )}
    </form>
    {/* Outside the form: a portal's submit would bubble into it. */}
    {customSchedule ? (
      <TimeDialog action="Schedule" onClose={() => setCustomSchedule(false)} onPick={schedule} title="Schedule message" />
    ) : null}
    </>
  )
}

export { StructuredComposer as Composer }
