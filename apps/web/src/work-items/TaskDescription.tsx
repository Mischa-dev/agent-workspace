import {
  useId,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type FocusEvent,
  type KeyboardEvent,
  type MouseEvent,
  type RefObject,
} from 'react'

import { Bold, Code, Italic, TextQuote } from '../files/icons'
import { Button, Icon, IconButton, Tooltip, type IconGlyph } from '../ui'
import { Markdown } from '../ui/markdown/Markdown'
import { applyEdit, formatEdit, type FormatAction } from '../ui/markdown/editing'
import { thingLink } from '../modules/references/thingLinks'
import { TaskNotice } from './TaskNotice'
import type { WorkItem } from './types'
import type { TaskSave } from './useTaskSave'

const mod = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent) ? '⌘' : 'Ctrl+'

// The same buttons as the editor on pages, with the glyphs the message box already loads.
const TOOLS: { action: FormatAction; label: string; icon: IconGlyph; shortcut?: string }[] = [
  { action: 'bold', label: 'Bold', icon: Bold, shortcut: `${mod}B` },
  { action: 'italic', label: 'Italic', icon: Italic, shortcut: `${mod}I` },
  { action: 'code', label: 'Code', icon: Code },
  { action: 'link', label: 'Link', icon: 'link', shortcut: `${mod}K` },
  { action: 'bullet', label: 'Bulleted list', icon: 'list' },
  { action: 'checklist', label: 'Checklist', icon: 'checklist' },
  { action: 'quote', label: 'Quote', icon: TextQuote },
]

const SHORTCUTS: Record<string, FormatAction> = { b: 'bold', i: 'italic', k: 'link' }

// Buttons inside the editor keep the text area focused, so pressing one never counts as leaving it.
const keepFocus = (event: MouseEvent) => event.preventDefault()

/**
 * The description, rendered as Markdown. A click on it (not on a link), its
 * pencil or E opens the editor; it saves when you click away, press
 * Ctrl/⌘+Enter or Escape. A save that meets someone else's edit keeps the
 * person's text in the editor until they keep it or take theirs.
 */
export function TaskDescription({
  disabled,
  editRef,
  item,
  save,
}: {
  disabled: boolean
  /** Set to a function that opens the editor, for the E shortcut. */
  editRef: RefObject<(() => void) | null>
  item: WorkItem
  save: TaskSave
}) {
  const pending = save.pending('description')
  const stopped = pending && pending.state !== 'saving' ? pending : null
  const shown = pending?.text ?? item.description
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState('')
  const [showTheirs, setShowTheirs] = useState(false)
  // What the description was when editing began, to tell whether someone else changed it meanwhile.
  const [base, setBase] = useState('')
  const focusOnOpen = useRef(false)
  const open = useRef(false)
  const wrapper = useRef<HTMLDivElement>(null)
  const textarea = useRef<HTMLTextAreaElement>(null)
  const returnTo = useRef<HTMLButtonElement>(null)
  const selection = useRef<[number, number] | null>(null)
  const id = useId()

  // A save that stops opens the editor again with the person's text, without taking their focus.
  const stopKey = stopped ? `${stopped.state}\n${stopped.text ?? ''}` : null
  const [seenStop, setSeenStop] = useState<string | null>(null)
  if (stopKey !== seenStop) {
    setSeenStop(stopKey)
    if (stopped && !editing) {
      setDraft(stopped.text ?? '')
      setEditing(true)
    }
  }

  const start = () => {
    if (disabled) return
    if (editing) {
      textarea.current?.focus()
      return
    }
    setBase(save.baseFor('description'))
    focusOnOpen.current = true
    setDraft(shown)
    setEditing(true)
  }
  useImperativeHandle(editRef, () => start)

  useLayoutEffect(() => {
    open.current = editing
    const element = textarea.current
    if (!editing || !element) return
    element.style.height = 'auto'
    element.style.height = `${element.scrollHeight + 2}px`
    if (focusOnOpen.current) {
      focusOnOpen.current = false
      element.focus({ preventScroll: true })
      element.setSelectionRange(element.value.length, element.value.length)
      element.scrollIntoView?.({ block: 'nearest' })
    }
    if (selection.current) {
      element.setSelectionRange(selection.current[0], selection.current[1])
      selection.current = null
    }
  }, [draft, editing])

  const close = (refocus: boolean) => {
    setEditing(false)
    setShowTheirs(false)
    if (refocus) window.requestAnimationFrame(() => returnTo.current?.focus({ preventScroll: true }))
  }

  const commit = (refocus = false) => {
    if (stopped) return
    close(refocus)
    if (draft !== shown) save.save('description', { description: draft }, base)
  }

  const cancel = () => {
    if (stopped) save.discard('description')
    close(true)
  }

  const onBlur = (event: FocusEvent<HTMLDivElement>) => {
    if (stopped) return
    const next = event.relatedTarget as Node | null
    if (next && wrapper.current?.contains(next)) return
    window.requestAnimationFrame(() => {
      // Switching to another window keeps the editor open; it saves once you click elsewhere here.
      if (!open.current || !document.hasFocus() || wrapper.current?.contains(document.activeElement)) return
      commit()
    })
  }

  const format = (action: FormatAction) => {
    const element = textarea.current
    if (!element) return
    const edit = formatEdit(action, { value: draft, start: element.selectionStart, end: element.selectionEnd })
    const next = applyEdit(draft, edit)
    if (next.length > 100_000) return
    element.focus()
    element.setSelectionRange(edit.from, edit.to)
    // insertText keeps the browser's undo history; fall back to a plain update where it isn't supported.
    let inserted: boolean
    try {
      inserted = typeof document.execCommand === 'function' && document.execCommand('insertText', false, edit.insert)
    } catch {
      inserted = false
    }
    if (!inserted || element.value !== next) setDraft(next)
    if (inserted && element.value === next) element.setSelectionRange(edit.selectStart, edit.selectEnd)
    else selection.current = [edit.selectStart, edit.selectEnd]
  }

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.nativeEvent.isComposing) return
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      commit(true)
      return
    }
    if (!(event.metaKey || event.ctrlKey) || event.altKey) return
    if (event.key === 'Enter') {
      event.preventDefault()
      commit(true)
      return
    }
    const action = event.shiftKey ? undefined : SHORTCUTS[event.key.toLowerCase()]
    if (!action) return
    event.preventDefault()
    format(action)
  }

  const onViewClick = (event: MouseEvent<HTMLDivElement>) => {
    if (disabled) return
    const target = event.target as HTMLElement
    if (target.closest('a, button, input, label, summary')) return
    // Selecting text to copy isn't asking to edit.
    if (window.getSelection()?.toString()) return
    start()
  }

  if (editing) {
    const theirs = stopped?.theirs?.description ?? ''
    // What the person saw is out of date: say so now rather than at save time.
    const changedMeanwhile = !stopped && item.description !== base
    return (
      <div className="aw-taskdesc aw-taskdesc--editing" onBlur={onBlur} ref={wrapper}>
        {stopped?.state === 'conflict' ? (
          <TaskNotice
            actions={
              <>
                <Button
                  onClick={() => {
                    save.keepMine('description', { description: draft })
                    close(true)
                  }}
                  onMouseDown={keepFocus}
                  size="sm"
                  variant="primary"
                >
                  Keep yours
                </Button>
                <Button
                  onClick={() => {
                    save.discard('description')
                    close(true)
                  }}
                  onMouseDown={keepFocus}
                  size="sm"
                >
                  Use theirs
                </Button>
                <Button aria-expanded={showTheirs} onClick={() => setShowTheirs(!showTheirs)} onMouseDown={keepFocus} size="sm" variant="ghost">
                  {showTheirs ? 'Hide their version' : 'See their version'}
                </Button>
              </>
            }
            icon="triangle-alert"
            live="alert"
            title="Someone else changed the description while you were editing."
            tone="warning"
          >
            Your text is still here. Keep yours to replace theirs, or use theirs and let yours go.
          </TaskNotice>
        ) : null}
        {stopped?.state === 'failed' ? (
          <TaskNotice
            actions={
              <>
                <Button
                  onClick={() => {
                    save.retry('description', { description: draft })
                    close(true)
                  }}
                  onMouseDown={keepFocus}
                  size="sm"
                  variant="primary"
                >
                  Try again
                </Button>
                <Button onClick={cancel} onMouseDown={keepFocus} size="sm">
                  Discard
                </Button>
              </>
            }
            icon="circle-alert"
            live="alert"
            title="Your description didn’t save."
            tone="danger"
          >
            {stopped.message}
          </TaskNotice>
        ) : null}
        {showTheirs && stopped ? (
          <div aria-label="Their version" className="aw-taskdesc__theirs" role="region">
            <Markdown empty={<p className="aw-taskdesc__placeholder">They left it empty.</p>} source={theirs} />
          </div>
        ) : null}
        <div className="aw-taskdesc__box">
          <textarea
            aria-label="Description"
            className="aw-taskdesc__input"
            id={id}
            maxLength={100_000}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={onKeyDown}
            placeholder="What should happen, any limits, and links that help."
            ref={textarea}
            rows={3}
            value={draft}
          />
          {changedMeanwhile ? (
            <p className="aw-taskdesc__note" role="status">
              Someone else just changed the description. Saving asks which version to keep.
            </p>
          ) : null}
          <div className="aw-taskdesc__foot">
            <div aria-controls={id} aria-label="Formatting" className="aw-taskdesc__tools" role="toolbar">
              {TOOLS.map(({ action, icon, label, shortcut }) => (
                <Tooltip key={action} label={label} shortcut={shortcut}>
                  <button aria-label={label} className="aw-iconbtn aw-iconbtn--sm" onClick={() => format(action)} onMouseDown={keepFocus} type="button">
                    <Icon name={icon} size={15} />
                  </button>
                </Tooltip>
              ))}
            </div>
            {stopped ? null : (
              <span className="aw-taskdesc__actions">
                <Button onClick={cancel} onMouseDown={keepFocus} size="sm" variant="ghost">
                  Cancel
                </Button>
                <Button onClick={() => commit(true)} onMouseDown={keepFocus} size="sm" variant="primary">
                  Save
                </Button>
              </span>
            )}
          </div>
        </div>
      </div>
    )
  }

  if (!shown.trim()) {
    if (disabled) return null
    return (
      <button className="aw-taskdesc__empty" onClick={start} ref={returnTo} type="button">
        Add a description…
      </button>
    )
  }

  return (
    <div className={disabled ? 'aw-taskdesc' : 'aw-taskdesc is-editable'} onClick={onViewClick}>
      <Markdown className="aw-taskdesc__body" renderLink={thingLink} source={shown} />
      {disabled ? null : (
        <IconButton className="aw-taskdesc__edit" icon="pencil" label="Edit description" onClick={start} ref={returnTo} shortcut="E" size="sm" />
      )}
    </div>
  )
}
