import { Editor, Extension, type ChainedCommands } from '@tiptap/core'
import { Collaboration } from '@tiptap/extension-collaboration'
import {
  Bold,
  Blocks,
  Code,
  Heading1,
  Heading2,
  Heading3,
  Italic,
  Link,
  List,
  ListChecks,
  ListOrdered,
  MessageSquarePlus,
  Minus,
  Quote,
  Redo2,
  SquareCode,
  Strikethrough,
  Table,
  Undo2,
  type LucideIcon,
} from 'lucide-react'
import {
  lazy,
  Suspense,
  useEffect,
  useEffectEvent,
  useId,
  useReducer,
  useRef,
  useState,
  useSyncExternalStore,
  type FormEvent,
  type ReactNode,
} from 'react'
import { createPortal } from 'react-dom'
import { useNavigate } from 'react-router-dom'
import * as Y from 'yjs'

import { lastHeardVersion, onPageChange } from '../../realtime/liveDocuments'
import { useOptionalRealtime } from '../../realtime/realtimeContext'
import { Banner, Button, Menu, TextField, Tooltip, describeError, showToast, type MenuEntry } from '../../ui'
import { formatShortcut } from '../../ui/shortcut'
import { PageEmbed } from '../embeds/PageEmbed'
import { canInsertTable, currentLink, inTasks, insertEmbed, insertTable, removeLink, setLink, toggleTasks } from './commands'
import { fromBase64, getPageDocument, sendPageUpdate, syncPageDocument, type PageDocument } from './documentApi'
import { EmbedSlots } from './embeds'
import { DiscussionMarks, discussionKey, selectionAnchor, type MarkedThread, type Marks } from './highlights'
import { FROM_SERVER, PageSync, saveLabel, type SaveFailure, type SaveState } from './pageSync'
import { pageExtensions } from './schema'
import './live.css'
import '../../styles/conversation.css'
import '../../styles/markdown.css'
import '../../styles/pages.css'

const CollectionEmbedPicker = lazy(() => import('../../collections/CollectionEmbedPicker'))
const EmbedPicker = lazy(() => import('../embeds/EmbedPicker'))
const SheetEmbedPicker = lazy(() => import('../../sheets/SheetEmbedPicker'))

// A page you write in place, live with everyone who has it open (AWD-53): a
// Tiptap editor on the page's Yjs document, kept in step with the server by
// PageSync. People and agents change the same document; the server merges
// every change and keeps the page's Markdown from it.

/** How often the page checks for changes while the live connection is down. */
const POLL_MS = 5_000

export type LivePageProps = {
  pageId: string
  /** The page's project, whose collections its views can come from. */
  projectId: string
  /** Whether the toolbar offers a collection's view to put in the page. */
  collections: boolean
  /** Whether the toolbar offers a sheet's range to put in the page. */
  sheets: boolean
  /** Names the editor for screen readers. */
  title: string
  /** The page as last saved: shown while the live copy loads, and if it can't. */
  fallback: ReactNode
  /** Comments and suggestions to show in the page, and what commenting does (AWD-53). */
  discussion?: LiveDiscussion | null
}

export type LiveDiscussion = {
  threads: MarkedThread[]
  active: string | null
  /** Blocks a pending suggestion changes. */
  suggested: string[]
  /** Start a thread about these words. */
  onComment: (anchor: { blockId: string; quote: string }) => void
  /** Someone clicked words a thread is about. */
  onOpenThread: (id: string) => void
}

type Load = { attempt: number; snapshot: PageDocument; doc: Y.Doc } | { attempt: number; error: unknown }

/** This browser's copy of the page's document, as the server sent it. */
function openDocument(snapshot: PageDocument) {
  const doc = new Y.Doc()
  // Client 1 writes for the server (apps/server/src/page_docs/ydoc.rs).
  while (doc.clientID <= 1) doc.clientID = crypto.getRandomValues(new Uint32Array(1))[0]
  Y.applyUpdate(doc, fromBase64(snapshot.state), FROM_SERVER)
  return doc
}

export default function LivePage({ collections, discussion = null, fallback, pageId, projectId, sheets, title }: LivePageProps) {
  const [attempt, setAttempt] = useState(0)
  const [load, setLoad] = useState<Load | null>(null)
  const [unshowable, setUnshowable] = useState(false)
  useEffect(() => {
    let alive = true
    getPageDocument(pageId)
      .then((snapshot) => {
        if (alive) setLoad({ attempt, snapshot, doc: openDocument(snapshot) })
      })
      .catch((error: unknown) => {
        if (alive) setLoad({ attempt, error })
      })
    return () => {
      alive = false
    }
  }, [attempt, pageId])
  const reload = () => setAttempt((value) => value + 1)
  const current = load?.attempt === attempt ? load : null
  if (unshowable) {
    return (
      <>
        <Banner title="This page holds something the editor can’t show yet,">
          so it’s shown as last saved. Edit details still changes it.
        </Banner>
        {fallback}
      </>
    )
  }
  if (current && 'error' in current) {
    return (
      <>
        <Banner onRetry={reload} title="Live editing didn’t load." tone="warning">
          {describeError(current.error).message} The page is shown as last saved.
        </Banner>
        {fallback}
      </>
    )
  }
  if (!current) {
    return (
      <div className="aw-live">
        <div className="aw-live__top">
          <div aria-hidden className="aw-live__bar" />
        </div>
        {fallback}
      </div>
    )
  }
  return (
    <LiveEditor
      collections={collections ? projectId : null}
      sheets={sheets ? projectId : null}
      discussion={discussion}
      doc={current.doc}
      key={current.attempt}
      onReload={reload}
      onUnshowable={() => setUnshowable(true)}
      snapshot={current.snapshot}
      title={title}
    />
  )
}

/** The page's own keys: a link, a list of tasks, a comment, and saving now. */
const PageKeys = Extension.create<{ onLink: () => void; onSave: () => void; onComment: () => boolean }>({
  name: 'pageKeys',

  addOptions() {
    return { onLink: () => undefined, onSave: () => undefined, onComment: () => false }
  },

  addKeyboardShortcuts() {
    return {
      'Mod-Alt-m': () => this.options.onComment(),
      'Mod-k': () => {
        this.options.onLink()
        return true
      },
      // Everything saves as you go; this just doesn't wait for the pause.
      'Mod-s': () => {
        this.options.onSave()
        return true
      },
      'Mod-Shift-9': () => toggleTasks(this.editor),
    }
  },
})

/** Starts a comment on the selected words, or says why it can't. */
function commentOnSelection(editor: Editor, discussion: LiveDiscussion) {
  const anchor = selectionAnchor(editor.state)
  if (anchor === 'empty') {
    showToast({ title: 'Select the words to comment on.' })
    return
  }
  if (anchor === 'unsaved') {
    showToast({ title: 'That part is still saving.', description: 'Try again in a moment.' })
    return
  }
  discussion.onComment(anchor)
}

function LiveEditor({
  collections,
  discussion,
  doc,
  onReload,
  onUnshowable,
  sheets,
  snapshot,
  title,
}: {
  /** The project whose collections the toolbar offers, if it does. */
  collections: string | null
  discussion: LiveDiscussion | null
  doc: Y.Doc
  onReload: () => void
  onUnshowable: () => void
  /** The project whose sheets the toolbar offers, if it does. */
  sheets: string | null
  snapshot: PageDocument
  title: string
}) {
  const navigate = useNavigate()
  const realtime = useOptionalRealtime()
  const live = realtime?.connection === 'live'
  const resync = realtime?.resyncGeneration ?? 0
  const mount = useRef<HTMLDivElement>(null)
  const sync = useRef<PageSync | null>(null)
  const editorId = useId()
  const [editor, setEditor] = useState<Editor | null>(null)
  const [saveState, setSaveState] = useState<SaveState>('saved')
  const [failure, setFailure] = useState<SaveFailure | null>(null)
  const [linking, setLinking] = useState(false)
  // Embedding by address, a collection view, or a sheet's range.
  const [embedding, setEmbedding] = useState<'address' | 'collection' | 'sheet' | null>(null)
  // Each embed's live view, drawn into the element the editor keeps for it.
  const [slots] = useState(() => new EmbedSlots())
  const embeds = useSyncExternalStore(slots.subscribe, slots.getSnapshot)

  const open = useEffectEvent((href: string) => {
    if (href.startsWith('/')) navigate(href)
    else window.open(href, '_blank', 'noopener,noreferrer')
  })
  const replaced = useEffectEvent(() => onReload())
  const unshowable = useEffectEvent(() => onUnshowable())
  const label = useEffectEvent(() => title)
  const openThread = useEffectEvent((id: string) => discussion?.onOpenThread(id))
  const comment = useEffectEvent((editor: Editor) => {
    if (!discussion) return false
    commentOnSelection(editor, discussion)
    return true
  })

  useEffect(() => {
    const element = mount.current
    if (!element) return
    const pageSync = new PageSync({
      doc,
      generation: snapshot.generation,
      version: snapshot.version,
      api: {
        sync: (body) => syncPageDocument(snapshot.pageId, body),
        send: (body) => sendPageUpdate(snapshot.pageId, body),
      },
      editable: snapshot.canEdit,
      onState: setSaveState,
      onFailure: setFailure,
      onReplaced: () => replaced(),
    })
    sync.current = pageSync
    const instance: Editor = new Editor({
      element,
      extensions: [
        ...pageExtensions({ onOpen: (href) => open(href), embeds: slots }),
        Collaboration.configure({ document: doc }),
        PageKeys.configure({
          onLink: () => setLinking(true),
          onSave: () => pageSync.flush(),
          onComment: (): boolean => comment(instance),
        }),
        DiscussionMarks.configure({ onOpenThread: (id) => openThread(id) }),
      ],
      editable: snapshot.canEdit,
      // Content this editor can't show stays in the document, untouched.
      enableContentCheck: true,
      injectCSS: false,
      editorProps: {
        attributes: { id: editorId, class: 'aw-md aw-doc aw-live__doc', 'aria-label': label() },
      },
      onContentError: () => unshowable(),
      onCreate: ({ editor: created }) => setEditor(created),
    })
    // Changes from others: the realtime channel says when, and anything
    // heard while this was loading is fetched now.
    const stopHearing = onPageChange(snapshot.pageId, (change) => pageSync.heard(change.version))
    pageSync.heard(lastHeardVersion(snapshot.pageId))
    const online = () => pageSync.resume()
    const hidden = () => {
      if (window.document.visibilityState === 'hidden') pageSync.flush()
    }
    const leaving = () => pageSync.flush()
    // Don't let a closing tab lose typing that hasn't reached the server.
    const warn = (event: BeforeUnloadEvent) => {
      if (pageSync.unsaved) event.preventDefault()
    }
    window.addEventListener('online', online)
    window.document.addEventListener('visibilitychange', hidden)
    window.addEventListener('pagehide', leaving)
    window.addEventListener('beforeunload', warn)
    return () => {
      window.removeEventListener('online', online)
      window.document.removeEventListener('visibilitychange', hidden)
      window.removeEventListener('pagehide', leaving)
      window.removeEventListener('beforeunload', warn)
      stopHearing()
      instance.destroy()
      // Sends what's waiting, then goes quiet.
      pageSync.destroy()
      sync.current = null
    }
  }, [doc, editorId, slots, snapshot])

  // The words open threads are about, and blocks with suggested edits.
  const threads = discussion?.threads
  const active = discussion?.active ?? null
  const suggested = discussion?.suggested
  useEffect(() => {
    if (!editor || editor.isDestroyed) return
    const marks: Marks = { threads: threads ?? [], active, suggested: suggested ?? [] }
    editor.view.dispatch(editor.state.tr.setMeta(discussionKey, marks).setMeta('addToHistory', false))
  }, [editor, threads, active, suggested])

  // After a reconnect, and on a timer while the live connection is down.
  useEffect(() => {
    if (resync > 0) sync.current?.refresh()
  }, [resync])
  useEffect(() => {
    if (live) return
    const timer = window.setInterval(() => sync.current?.refresh(), POLL_MS)
    return () => window.clearInterval(timer)
  }, [live])

  const status = snapshot.canEdit ? saveLabel(saveState) : 'View only'
  return (
    <div className="aw-live">
      {failure ? (
        <Banner
          action={
            <span className="aw-live__actions">
              <Button onClick={failure.retry} size="sm">
                Try again
              </Button>
              <Button onClick={onReload} size="sm" variant="ghost">
                Reload page
              </Button>
            </span>
          }
          title="Changes aren’t saving."
          tone="danger"
        >
          {failure.message} Reloading shows the saved page and drops what didn’t save.
        </Banner>
      ) : null}
      <div className="aw-live__top">
        <div className="aw-live__bar">
          {snapshot.canEdit ? (
            editor ? (
              <Toolbar
                controls={editorId}
                editor={editor}
                onComment={discussion ? () => commentOnSelection(editor, discussion) : null}
                onEmbed={() => {
                  setLinking(false)
                  setEmbedding('address')
                }}
                onLink={() => {
                  setEmbedding(null)
                  setLinking(true)
                }}
              />
            ) : (
              <div className="aw-live__tools" />
            )
          ) : null}
          <span aria-live="polite" className="aw-live__save" data-state={snapshot.canEdit ? saveState : 'reading'} title={status}>
            {status}
          </span>
        </div>
        {linking && editor ? <LinkForm editor={editor} onClose={() => setLinking(false)} /> : null}
        {embedding === 'address' && editor ? (
          <Suspense fallback={null}>
            <EmbedPicker
              onClose={() => {
                setEmbedding(null)
                editor.commands.focus()
              }}
              onCollection={collections ? () => setEmbedding('collection') : null}
              onSheet={sheets ? () => setEmbedding('sheet') : null}
              onPick={(embed) => {
                insertEmbed(editor, embed)
                setEmbedding(null)
              }}
            />
          </Suspense>
        ) : null}
        {embedding === 'collection' && editor && collections ? (
          <Suspense fallback={null}>
            <CollectionEmbedPicker
              onClose={() => {
                setEmbedding(null)
                editor.commands.focus()
              }}
              onPick={(embed) => {
                insertEmbed(editor, embed)
                setEmbedding(null)
              }}
              projectId={collections}
            />
          </Suspense>
        ) : null}
        {embedding === 'sheet' && editor && sheets ? (
          <Suspense fallback={null}>
            <SheetEmbedPicker
              onClose={() => {
                setEmbedding(null)
                editor.commands.focus()
              }}
              onPick={(embed) => {
                insertEmbed(editor, embed)
                setEmbedding(null)
              }}
              projectId={sheets}
            />
          </Suspense>
        ) : null}
      </div>
      <div className="aw-live__page" ref={mount} />
      {embeds.map((slot) => createPortal(<PageEmbed framed={false} href={slot.href} label={slot.label} />, slot.element, slot.key))}
    </div>
  )
}

type Tool = {
  key: string
  label: string
  icon: LucideIcon
  shortcut?: string
  run: (editor: Editor) => unknown
  active?: (editor: Editor) => boolean
  enabled?: (editor: Editor) => boolean
}

const heading = (level: 1 | 2 | 3, icon: LucideIcon): Tool => ({
  key: `h${level}`,
  label: `Heading ${level}`,
  icon,
  shortcut: `mod+alt+${level}`,
  run: (editor) => editor.chain().focus().toggleHeading({ level }).run(),
  active: (editor) => editor.isActive('heading', { level }),
  enabled: (editor) => editor.can().toggleHeading({ level }),
})

const TOOLS: Tool[][] = [
  [heading(1, Heading1), heading(2, Heading2), heading(3, Heading3)],
  [
    {
      key: 'bold',
      label: 'Bold',
      icon: Bold,
      shortcut: 'mod+b',
      run: (editor) => editor.chain().focus().toggleBold().run(),
      active: (editor) => editor.isActive('bold'),
      enabled: (editor) => editor.can().toggleBold(),
    },
    {
      key: 'italic',
      label: 'Italic',
      icon: Italic,
      shortcut: 'mod+i',
      run: (editor) => editor.chain().focus().toggleItalic().run(),
      active: (editor) => editor.isActive('italic'),
      enabled: (editor) => editor.can().toggleItalic(),
    },
    {
      key: 'strike',
      label: 'Strikethrough',
      icon: Strikethrough,
      shortcut: 'mod+shift+s',
      run: (editor) => editor.chain().focus().toggleStrike().run(),
      active: (editor) => editor.isActive('strike'),
      enabled: (editor) => editor.can().toggleStrike(),
    },
    {
      key: 'code',
      label: 'Code',
      icon: Code,
      shortcut: 'mod+e',
      run: (editor) => editor.chain().focus().toggleCode().run(),
      active: (editor) => editor.isActive('code'),
      enabled: (editor) => editor.can().toggleCode(),
    },
    // Opens the link field: see Toolbar.
    {
      key: 'link',
      label: 'Link',
      icon: Link,
      shortcut: 'mod+k',
      run: () => undefined,
      active: (editor) => editor.isActive('link'),
      enabled: (editor) => editor.can().setMark('link', { href: '/' }),
    },
  ],
  [
    {
      key: 'bullets',
      label: 'Bulleted list',
      icon: List,
      shortcut: 'mod+shift+8',
      run: (editor) => editor.chain().focus().toggleBulletList().run(),
      active: (editor) => editor.isActive('bulletList') && !inTasks(editor.state),
      enabled: (editor) => editor.can().toggleBulletList(),
    },
    {
      key: 'numbers',
      label: 'Numbered list',
      icon: ListOrdered,
      shortcut: 'mod+shift+7',
      run: (editor) => editor.chain().focus().toggleOrderedList().run(),
      active: (editor) => editor.isActive('orderedList') && !inTasks(editor.state),
      enabled: (editor) => editor.can().toggleOrderedList(),
    },
    {
      key: 'tasks',
      label: 'Checklist',
      icon: ListChecks,
      shortcut: 'mod+shift+9',
      run: (editor) => toggleTasks(editor),
      active: (editor) => inTasks(editor.state),
      enabled: (editor) => inTasks(editor.state) || editor.can().toggleBulletList(),
    },
  ],
  [
    {
      key: 'quote',
      label: 'Quote',
      icon: Quote,
      shortcut: 'mod+shift+b',
      run: (editor) => editor.chain().focus().toggleBlockquote().run(),
      active: (editor) => editor.isActive('blockquote'),
      enabled: (editor) => editor.can().toggleBlockquote(),
    },
    {
      key: 'codeBlock',
      label: 'Code block',
      icon: SquareCode,
      shortcut: 'mod+alt+c',
      run: (editor) => editor.chain().focus().toggleCodeBlock().run(),
      active: (editor) => editor.isActive('codeBlock'),
      enabled: (editor) => editor.can().toggleCodeBlock(),
    },
    // In a table this is the table's menu: see Toolbar.
    {
      key: 'table',
      label: 'Table',
      icon: Table,
      run: (editor) => insertTable(editor),
      active: (editor) => editor.isActive('table'),
      enabled: (editor) => editor.isActive('table') || canInsertTable(editor),
    },
    {
      key: 'rule',
      label: 'Divider',
      icon: Minus,
      run: (editor) => editor.chain().focus().setHorizontalRule().run(),
      enabled: (editor) => editor.can().setHorizontalRule(),
    },
    // Opens the embed picker: see Toolbar.
    {
      key: 'embed',
      label: 'Embed',
      icon: Blocks,
      run: () => undefined,
    },
  ],
  [
    // Starts a comment on the selection: see Toolbar.
    {
      key: 'comment',
      label: 'Comment',
      icon: MessageSquarePlus,
      shortcut: 'mod+alt+m',
      run: () => undefined,
      enabled: (editor) => !editor.state.selection.empty,
    },
  ],
  [
    {
      key: 'undo',
      label: 'Undo',
      icon: Undo2,
      shortcut: 'mod+z',
      run: (editor) => editor.chain().focus().undo().run(),
      enabled: (editor) => editor.can().undo(),
    },
    {
      key: 'redo',
      label: 'Redo',
      icon: Redo2,
      shortcut: 'mod+shift+z',
      run: (editor) => editor.chain().focus().redo().run(),
      enabled: (editor) => editor.can().redo(),
    },
  ],
]

function tableMenu(editor: Editor): MenuEntry[] {
  const run = (change: (chain: ChainedCommands) => ChainedCommands) => () => {
    change(editor.chain().focus()).run()
  }
  return [
    { label: 'Add row above', onSelect: run((chain) => chain.addRowBefore()) },
    { label: 'Add row below', onSelect: run((chain) => chain.addRowAfter()) },
    { label: 'Add column left', onSelect: run((chain) => chain.addColumnBefore()) },
    { label: 'Add column right', onSelect: run((chain) => chain.addColumnAfter()) },
    { separator: true },
    { label: 'Delete row', disabled: !editor.can().deleteRow(), onSelect: run((chain) => chain.deleteRow()) },
    { label: 'Delete column', disabled: !editor.can().deleteColumn(), onSelect: run((chain) => chain.deleteColumn()) },
    { label: 'Delete table', danger: true, onSelect: run((chain) => chain.deleteTable()) },
  ]
}

/** The formatting buttons, kept in step with where the cursor is. */
function Toolbar({
  controls,
  editor,
  onComment,
  onEmbed,
  onLink,
}: {
  controls: string
  editor: Editor
  onComment: (() => void) | null
  onEmbed: (() => void) | null
  onLink: () => void
}) {
  const [, redraw] = useReducer((count: number) => count + 1, 0)
  useEffect(() => {
    editor.on('transaction', redraw)
    return () => {
      editor.off('transaction', redraw)
    }
  }, [editor])
  return (
    <div aria-controls={controls} aria-label="Formatting" className="aw-live__tools" role="toolbar">
      {TOOLS.map((group) => (
        <div className="aw-live__group" hidden={group[0].key === 'comment' && !onComment} key={group[0].key}>
          {group.map((tool) => {
            if (tool.key === 'embed' && !onEmbed) return null
            if (tool.key === 'comment' && !onComment) return null
            const Glyph = tool.icon
            const pressed = tool.active?.(editor)
            const button = (
              <button
                aria-label={tool.label}
                aria-pressed={tool.active && tool.key !== 'table' ? pressed : undefined}
                className="aw-iconbtn aw-iconbtn--sm"
                disabled={tool.enabled ? !tool.enabled(editor) : false}
                onClick={
                  tool.key === 'link'
                    ? onLink
                    : tool.key === 'embed' && onEmbed
                      ? onEmbed
                      : tool.key === 'comment' && onComment
                        ? onComment
                        : () => tool.run(editor)
                }
                // Keep the page's selection when a button is pressed.
                onMouseDown={(event) => event.preventDefault()}
                type="button"
              >
                <Glyph aria-hidden size={16} />
              </button>
            )
            if (tool.key === 'table' && pressed) {
              return <Menu items={tableMenu(editor)} key={tool.key} label="Table" trigger={button} />
            }
            return (
              <Tooltip key={tool.key} label={tool.label} shortcut={tool.shortcut ? formatShortcut(tool.shortcut) : undefined}>
                {button}
              </Tooltip>
            )
          })}
        </div>
      ))}
    </div>
  )
}

/** Where a link goes: added to the selection, or the link the cursor is on changed or taken off. */
function LinkForm({ editor, onClose }: { editor: Editor; onClose: () => void }) {
  const [existing] = useState(() => currentLink(editor))
  const [address, setAddress] = useState(existing ?? '')
  const [error, setError] = useState<string | null>(null)
  const close = () => {
    onClose()
    editor.commands.focus()
  }
  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (!setLink(editor, address)) {
      setError('Use a web address (https://…), an email address (mailto:…) or a path in this workspace (/…).')
      return
    }
    onClose()
  }
  return (
    <form
      className="aw-live__link"
      onKeyDown={(event) => {
        if (event.key !== 'Escape') return
        event.preventDefault()
        close()
      }}
      onSubmit={submit}
    >
      <TextField
        autoFocus
        className="aw-live__address"
        compact
        error={error}
        label="Link address"
        onChange={(event) => {
          setAddress(event.target.value)
          setError(null)
        }}
        placeholder="https://example.com"
        value={address}
      />
      <span className="aw-live__actions">
        <Button size="sm" type="submit" variant="primary">
          {existing ? 'Change link' : 'Add link'}
        </Button>
        {existing ? (
          <Button
            onClick={() => {
              removeLink(editor)
              onClose()
            }}
            size="sm"
          >
            Remove link
          </Button>
        ) : null}
        <Button onClick={close} size="sm" variant="ghost">
          Cancel
        </Button>
      </span>
    </form>
  )
}
