import '@excalidraw/excalidraw/index.css'

import { useCallback, useEffect, useRef, useState, type MutableRefObject } from 'react'
import { CaptureUpdateAction, Excalidraw, reconcileElements, restoreElements } from '@excalidraw/excalidraw'
import type { OrderedExcalidrawElement } from '@excalidraw/excalidraw/element/types'
import type { AppState, BinaryFileData, BinaryFiles, Collaborator, ExcalidrawImperativeAPI, SocketId } from '@excalidraw/excalidraw/types'
import { useNavigate } from 'react-router-dom'

import { useOptionalRealtime, useWatchDocument } from '../realtime/realtimeContext'
import { onWhiteboardChange } from '../realtime/liveDocuments'
import type { Outline } from '../changes/recentEdits'
import { sameIds, selectionBounds, type BoardSelection, type BoardViewport } from './pins'
import { describeError } from '../ui'
import { getFile, getScene, uploadFile, writeScene, type Editor, type SceneView, type StoredElement } from './api'
import { missingFiles, remember, unsaved, type Known, type SaveState } from './sync'

/** How long the board waits after the last change before saving it. */
const SAVE_DELAY_MS = 250
/** How often the board checks for changes while the live connection is down. */
const POLL_MS = 5_000
/** A failed save is tried again after 3, then 6 seconds; after that the page says why and waits for you. */
const RETRY_MS = 3_000
const RETRIES = 2

/** Why saving stopped, and how to try again. */
export type SaveFailure = { message: string; retry: () => void }

/** In suggest mode (C1.11): the drawing not yet sent, and a way to drop it. */
export type DraftHandle = {
  /** The elements changed here since the server's copy, to send as a proposal. */
  draft: () => StoredElement[]
  /**
   * The server's version of each drafted element when drawing on it began
   * (0 for a new one), so a change made there since is caught.
   */
  readVersions: () => Record<string, number>
  /** Puts the board back as the server holds it. */
  discard: () => Promise<void>
}

export type BoardCanvasProps = {
  whiteboardId: string
  initial: SceneView
  editable: boolean
  dark: boolean
  onSaveState: (state: SaveState) => void
  onEditors: (editors: Editor[]) => void
  /** Saving has failed more than a retry can fix (null once it saves again). */
  onSaveFailure?: (failure: SaveFailure | null) => void
  /** The role only suggests: drawing stays here until it's sent as a proposal. */
  suggesting?: boolean
  /** The drawing is being sent: hold it still so nothing drawn now is lost. */
  paused?: boolean
  /** How many elements the unsent drawing changes. */
  onDraft?: (count: number) => void
  draftRef?: MutableRefObject<DraftHandle | null>
  /** Elements someone changed recently, by id (C1.10): outlined in their name. */
  edited?: ReadonlyMap<string, Outline>
  /** The selection changed (C1.31): its elements and their box, or null. */
  onSelection?: (selection: BoardSelection | null) => void
  /** The board scrolled or zoomed, to place comment pins over it. */
  onViewport?: (viewport: BoardViewport) => void
}

type Element = OrderedExcalidrawElement

const asStored = (elements: readonly Element[]) => elements as unknown as StoredElement[]
const asElements = (elements: readonly StoredElement[]) => elements as unknown as Element[]

export default function BoardCanvas({
  whiteboardId,
  initial,
  editable,
  dark,
  onSaveState,
  onEditors,
  onSaveFailure,
  suggesting = false,
  paused = false,
  onDraft,
  draftRef,
  edited,
  onSelection,
  onViewport,
}: BoardCanvasProps) {
  const navigate = useNavigate()
  const realtime = useOptionalRealtime()
  const live = realtime?.connection === 'live'
  const resync = realtime?.resyncGeneration ?? 0
  // In the board's live room while it is open (W1.6).
  useWatchDocument(whiteboardId ? { kind: 'whiteboard', id: whiteboardId } : null)
  const api = useRef<ExcalidrawImperativeAPI | null>(null)
  // What the server holds starts as the scene that came with the page.
  const [known] = useState<Known>(() => {
    const seeded: Known = new Map()
    remember(seeded, initial.elements)
    return seeded
  })
  const seq = useRef(initial.sceneSeq)
  // Suggest mode: each drafted element's server version when drawing on it began.
  const baseline = useRef(new Map<string, number>())
  const loadedFiles = useRef(new Set<string>())
  const uploadedFiles = useRef(new Set(initial.files.map((file) => file.id)))
  const saveTimer = useRef<number | undefined>(undefined)
  const saving = useRef(false)
  const fetching = useRef<Promise<void> | null>(null)
  const fetchAgain = useRef(false)
  const failed = useRef(false)
  const failures = useRef(0)
  const retryTimer = useRef<number | undefined>(undefined)
  // Once the board closes, a last save may still go out, but nothing retries.
  const closed = useRef(false)
  const callbacks = useRef({ onSaveState, onEditors, onSaveFailure, onDraft, onSelection, onViewport })
  const saveRef = useRef<() => Promise<void>>(() => Promise.resolve())
  useEffect(() => {
    callbacks.current = { onSaveState, onEditors, onSaveFailure, onDraft, onSelection, onViewport }
  }, [onDraft, onEditors, onSaveFailure, onSaveState, onSelection, onViewport])
  useEffect(() => {
    closed.current = false
    return () => {
      closed.current = true
    }
  }, [])

  const loadFiles = useCallback(
    async (elements: readonly StoredElement[]) => {
      const wanted = missingFiles(elements, loadedFiles.current)
      if (wanted.length === 0) return
      for (const id of wanted) loadedFiles.current.add(id)
      const files = await Promise.all(
        wanted.map((id) =>
          getFile(whiteboardId, id).catch(() => {
            loadedFiles.current.delete(id)
            return null
          }),
        ),
      )
      const found = files.filter((file) => file !== null)
      if (found.length) api.current?.addFiles(found as unknown as BinaryFileData[])
    },
    [whiteboardId],
  )

  // Bring in what changed on the server since the last look.
  const pull = useCallback(() => {
    if (fetching.current) {
      fetchAgain.current = true
      return fetching.current
    }
    const run = async () => {
      do {
        fetchAgain.current = false
        const changes = await getScene(whiteboardId, seq.current)
        callbacks.current.onEditors(changes.recentEditors)
        if (changes.sceneSeq < seq.current) continue
        seq.current = changes.sceneSeq
        const board = api.current
        if (!board || changes.elements.length === 0) continue
        const remote = restoreElements(asElements(changes.elements), null)
        remember(known, changes.elements)
        const merged = reconcileElements(
          board.getSceneElementsIncludingDeleted() as Element[],
          remote as unknown as Parameters<typeof reconcileElements>[1],
          board.getAppState() as AppState,
        )
        board.updateScene({ elements: merged, captureUpdate: CaptureUpdateAction.NEVER })
        void loadFiles(changes.elements)
      } while (fetchAgain.current)
    }
    fetching.current = run()
      .catch(() => undefined)
      .finally(() => {
        fetching.current = null
      })
    return fetching.current
  }, [known, loadFiles, whiteboardId])

  const save = useCallback(async () => {
    const board = api.current
    if (!board || !editable || saving.current) return
    const elements = asStored(board.getSceneElementsIncludingDeleted())
    const changed = unsaved(elements, known)
    const files = board.getFiles() as BinaryFiles
    const newFiles = Object.values(files).filter((file) => !uploadedFiles.current.has(file.id))
    if (changed.length === 0 && newFiles.length === 0) {
      callbacks.current.onSaveState(failed.current ? (failures.current > RETRIES ? 'failed' : 'error') : 'saved')
      return
    }
    saving.current = true
    callbacks.current.onSaveState(navigator.onLine === false ? 'offline' : 'saving')
    try {
      // Images first, so nobody sees an element whose image isn't there yet.
      for (const file of newFiles) {
        await uploadFile(whiteboardId, { id: file.id, mimeType: file.mimeType, dataURL: file.dataURL })
        uploadedFiles.current.add(file.id)
        loadedFiles.current.add(file.id)
      }
      for (let start = 0; start < changed.length; start += 1_000) {
        const batch = changed.slice(start, start + 1_000)
        const outcome = await writeScene(whiteboardId, batch)
        const accepted = new Set(outcome.accepted)
        remember(
          known,
          batch.filter((element) => accepted.has(element.id)),
        )
        // Someone else's newer copy won: take it, as the server did.
        if (outcome.rejected.length) {
          remember(known, outcome.rejected)
          const current = api.current
          if (current) {
            const merged = reconcileElements(
              current.getSceneElementsIncludingDeleted() as Element[],
              restoreElements(asElements(outcome.rejected), null) as unknown as Parameters<typeof reconcileElements>[1],
              current.getAppState() as AppState,
            )
            current.updateScene({ elements: merged, captureUpdate: CaptureUpdateAction.NEVER })
          }
        }
      }
      failed.current = false
      if (failures.current) {
        failures.current = 0
        callbacks.current.onSaveFailure?.(null)
      }
    } catch (reason) {
      failed.current = true
      window.clearTimeout(retryTimer.current)
      if (closed.current) return
      if (navigator.onLine === false) {
        // Coming back online saves again; nothing to retry until then.
        callbacks.current.onSaveState('offline')
        return
      }
      failures.current += 1
      if (failures.current <= RETRIES) {
        callbacks.current.onSaveState('error')
        retryTimer.current = window.setTimeout(() => void saveRef.current(), RETRY_MS * failures.current)
      } else {
        callbacks.current.onSaveState('failed')
        callbacks.current.onSaveFailure?.({
          message: describeError(reason).message,
          retry: () => {
            failures.current = 0
            void saveRef.current()
          },
        })
      }
      return
    } finally {
      saving.current = false
    }
    // Anything drawn while that was saving goes next.
    const again = unsaved(asStored(api.current?.getSceneElementsIncludingDeleted() ?? []), known)
    if (again.length) void saveRef.current()
    else callbacks.current.onSaveState('saved')
  }, [editable, known, whiteboardId])
  useEffect(() => {
    saveRef.current = save
  }, [save])

  // Suggest mode: the drawing is the elements changed here, sent on request.
  useEffect(() => {
    if (!draftRef) return
    draftRef.current = {
      draft: () => unsaved(asStored(api.current?.getSceneElementsIncludingDeleted() ?? []), known),
      readVersions: () => {
        const drafted = unsaved(asStored(api.current?.getSceneElementsIncludingDeleted() ?? []), known)
        return Object.fromEntries(drafted.map((element) => [element.id, baseline.current.get(element.id) ?? known.get(element.id)?.version ?? 0]))
      },
      discard: async () => {
        baseline.current.clear()
        const scene = await getScene(whiteboardId)
        remember(known, scene.elements)
        seq.current = Math.max(seq.current, scene.sceneSeq)
        api.current?.updateScene({ elements: restoreElements(asElements(scene.elements), null), captureUpdate: CaptureUpdateAction.NEVER })
        callbacks.current.onDraft?.(0)
      },
    }
    return () => {
      draftRef.current = null
    }
  }, [draftRef, known, whiteboardId])

  // What's selected and where the board is, told only when they change.
  const lastSelection = useRef<string[]>([])
  const lastViewport = useRef<BoardViewport | null>(null)
  const watch = useCallback((elements: readonly Element[], appState: AppState) => {
    const ids = Object.keys(appState.selectedElementIds)
      .filter((id) => appState.selectedElementIds[id])
      .sort()
    if (!sameIds(ids, lastSelection.current)) {
      lastSelection.current = ids
      callbacks.current.onSelection?.(ids.length ? selectionBounds(elements, ids) : null)
    }
    const viewport = { scrollX: appState.scrollX, scrollY: appState.scrollY, zoom: appState.zoom.value }
    const last = lastViewport.current
    if (!last || last.scrollX !== viewport.scrollX || last.scrollY !== viewport.scrollY || last.zoom !== viewport.zoom) {
      lastViewport.current = viewport
      callbacks.current.onViewport?.(viewport)
    }
  }, [])

  const onChange = useCallback(
    (elements: readonly Element[], appState: AppState) => {
      watch(elements, appState)
      if (suggesting) {
        const drafted = unsaved(asStored(elements), known)
        const ids = new Set(drafted.map((element) => element.id))
        // An element the server's copy caught up with is no longer drafted.
        for (const id of baseline.current.keys()) if (!ids.has(id)) baseline.current.delete(id)
        for (const element of drafted) {
          if (!baseline.current.has(element.id)) baseline.current.set(element.id, known.get(element.id)?.version ?? 0)
        }
        callbacks.current.onDraft?.(drafted.length)
        return
      }
      if (!editable) return
      if (unsaved(asStored(elements), known).length === 0 && !hasNewFiles(api.current, uploadedFiles.current)) return
      callbacks.current.onSaveState('unsaved')
      window.clearTimeout(saveTimer.current)
      saveTimer.current = window.setTimeout(() => void save(), SAVE_DELAY_MS)
    },
    [editable, known, save, suggesting, watch],
  )

  // Live changes from people and agents.
  useEffect(
    () =>
      onWhiteboardChange(whiteboardId, (change) => {
        if (change.sceneSeq > seq.current) void pull()
      }),
    [pull, whiteboardId],
  )
  // After a reconnect, and on a timer while the live connection is down.
  useEffect(() => {
    if (resync > 0) void pull()
  }, [pull, resync])
  useEffect(() => {
    if (live) return
    const timer = window.setInterval(() => void pull(), POLL_MS)
    return () => window.clearInterval(timer)
  }, [live, pull])
  useEffect(() => {
    const online = () => {
      void save()
      void pull()
    }
    window.addEventListener('online', online)
    return () => window.removeEventListener('online', online)
  }, [pull, save])
  // Don't let a closing tab lose a drawing that hasn't saved.
  useEffect(() => {
    const warn = (event: BeforeUnloadEvent) => {
      const board = api.current
      if (board && (editable || suggesting) && unsaved(asStored(board.getSceneElementsIncludingDeleted()), known).length) {
        event.preventDefault()
      }
    }
    window.addEventListener('beforeunload', warn)
    return () => {
      window.removeEventListener('beforeunload', warn)
      window.clearTimeout(saveTimer.current)
      window.clearTimeout(retryTimer.current)
      void save()
    }
  }, [editable, known, save, suggesting])

  // Recent edits (C1.10): the board outlines another person's selection in
  // their colour and name, so each author's changed elements show as theirs.
  const showEdited = useCallback(() => {
    const board = api.current
    if (!board) return
    const byActor = new Map<string, { username: string; ids: Record<string, true> }>()
    for (const [id, outline] of edited ?? []) {
      const entry = byActor.get(outline.actorId) ?? { username: outline.actor, ids: {} }
      entry.ids[id] = true
      byActor.set(outline.actorId, entry)
    }
    const collaborators = new Map<SocketId, Collaborator>(
      [...byActor].map(([actorId, entry]) => [`edited:${actorId}` as SocketId, { id: actorId, username: entry.username, selectedElementIds: entry.ids }]),
    )
    board.updateScene({ collaborators, captureUpdate: CaptureUpdateAction.NEVER })
  }, [edited])
  useEffect(() => showEdited(), [showEdited])

  return (
    <Excalidraw
      excalidrawAPI={(instance) => {
        api.current = instance
        void loadFiles(initial.elements)
        showEdited()
      }}
      initialData={{
        elements: restoreElements(asElements(initial.elements), null),
        scrollToContent: true,
      }}
      isCollaborating
      onChange={onChange}
      onLinkOpen={(element, event) => {
        const link = element.link
        if (link && link.startsWith('/') && !link.startsWith('//')) {
          event.preventDefault()
          navigate(link)
        }
      }}
      theme={dark ? 'dark' : 'light'}
      UIOptions={{
        canvasActions: { loadScene: false, saveToActiveFile: false, toggleTheme: false, export: false },
      }}
      validateEmbeddable={false}
      viewModeEnabled={paused || (!editable && !suggesting)}
    />
  )
}

function hasNewFiles(board: ExcalidrawImperativeAPI | null, uploaded: ReadonlySet<string>) {
  if (!board) return false
  return Object.keys(board.getFiles()).some((id) => !uploaded.has(id))
}
