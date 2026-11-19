import { useCallback, useEffect, useRef } from 'react'
import { useQueryClient, type InfiniteData } from '@tanstack/react-query'

import { isApiError } from '../api/errors'
import { toastError } from '../app/toastError'
import type { FieldReference, FieldValue } from '../fields/fieldModel'
import {
  collectionKeys,
  createRecord,
  getRecord,
  updateRecord,
  type CollectionRecord,
  type RecordBody,
  type RecordChange,
  type RecordPage,
} from './api'
import { recordName, withValues } from './collectionModel'
import { forget, inOrder, remember, type Undoable } from '../views/saveQueue'

function mergeReferences(current: readonly FieldReference[], added: readonly FieldReference[]) {
  const fresh = added.filter((reference) => !current.some((entry) => entry.id === reference.id && entry.kind === reference.kind))
  return fresh.length ? [...current, ...fresh] : (current as FieldReference[])
}

/** A record as it reads with `change` made. */
function withChange(record: CollectionRecord, change: RecordChange): CollectionRecord {
  return {
    ...record,
    title: change.title ?? record.title,
    values: withValues(record.values, change.values),
    archived: change.archived ?? record.archived,
  }
}

/** What `change` replaces on `record`: the change that takes it back. */
function inverseOf(record: CollectionRecord, change: RecordChange): RecordChange {
  return {
    ...(change.title !== undefined ? { title: record.title } : {}),
    ...(change.values ? { values: Object.fromEntries(Object.keys(change.values).map((id) => [id, record.values[id] ?? null])) } : {}),
    ...(change.archived !== undefined ? { archived: record.archived } : {}),
  }
}

/** The parts of `record` that `change` touches, to see whether they're still as a save left them. */
function touched(record: CollectionRecord, change: RecordChange) {
  return JSON.stringify([
    change.title !== undefined ? record.title : null,
    Object.keys(change.values ?? {}).map((id) => record.values[id] ?? null),
    change.archived !== undefined ? record.archived : null,
  ])
}

/**
 * Changes to a collection's records from the table and the record panel.
 * Each shows at once, then saves; one record's changes go one at a time, each
 * against the version the last one left, and a change that meets someone
 * else's newer version is sent again on top of it, since each names only
 * the values it sets. Until a change saves, it shows on top of whatever the
 * server says, so an answer to an earlier change, or a refetch, never takes
 * it away. A change that fails says so and the list refetches.
 */
export function useRecordEdits(projectId: string, collectionId: string) {
  const client = useQueryClient()
  const versions = useRef(new Map<string, number>())
  /** Changes not saved yet, by record, oldest first. */
  const unsaved = useRef(new Map<string, RecordChange[]>())
  const withUnsaved = useCallback((record: CollectionRecord) => (unsaved.current.get(record.id) ?? []).reduce(withChange, record), [])

  const patch = useCallback(
    (recordId: string, change: (record: CollectionRecord) => CollectionRecord, references: readonly FieldReference[] = []) => {
      client.setQueriesData<InfiniteData<RecordPage>>({ queryKey: collectionKeys.allRecords(projectId, collectionId) }, (data) =>
        data
          ? {
              ...data,
              pages: data.pages.map((page) => ({
                ...page,
                references: mergeReferences(page.references, references),
                records: page.records.map((record) => (record.id === recordId ? change(record) : record)),
              })),
            }
          : data,
      )
      client.setQueryData<RecordBody>(collectionKeys.record(projectId, collectionId, recordId), (body) =>
        body ? { ...body, references: mergeReferences(body.references, references), record: change(body.record) } : body,
      )
    },
    [client, collectionId, projectId],
  )

  const refresh = useCallback(() => client.invalidateQueries({ queryKey: collectionKeys.one(projectId, collectionId) }), [client, collectionId, projectId])

  // A list or record that loads while changes are saving shows them too.
  useEffect(() => {
    const scope = collectionKeys.one(projectId, collectionId)
    return client.getQueryCache().subscribe((event) => {
      // Our own patches come as manual updates; only what the server sent needs them on top.
      if (event.type !== 'updated' || event.action.type !== 'success' || event.action.manual || !unsaved.current.size) return
      const key = event.query.queryKey
      if (!scope.every((part, index) => key[index] === part) || (key[scope.length] !== 'records' && key[scope.length] !== 'record')) return
      for (const recordId of unsaved.current.keys()) patch(recordId, withUnsaved)
    })
  }, [client, collectionId, patch, projectId, withUnsaved])

  // Undo saves through `save` itself, which it can only name once made.
  const saveRef = useRef<((record: CollectionRecord, change: RecordChange, reference?: FieldReference, undoable?: boolean) => Promise<boolean>) | null>(null)

  /**
   * Saves a change to one record. `reference` names a person, task or page
   * just picked, so the cell shows it before the save lands. Once saved,
   * it's the change Undo takes back (T1.27), unless it is itself an undo.
   */
  const save = useCallback(
    (record: CollectionRecord, change: RecordChange, reference?: FieldReference, undoable = true): Promise<boolean> => {
      unsaved.current.set(record.id, [...(unsaved.current.get(record.id) ?? []), change])
      const settled = () => {
        const rest = (unsaved.current.get(record.id) ?? []).filter((entry) => entry !== change)
        if (rest.length) unsaved.current.set(record.id, rest)
        else unsaved.current.delete(record.id)
      }
      patch(record.id, (current) => withChange(current, change), reference ? [reference] : [])
      const send = async () => {
        const version = Math.max(record.version, versions.current.get(record.id) ?? 0)
        try {
          return await updateRecord(projectId, collectionId, record.id, version, change)
        } catch (error) {
          // An undo goes only on the version it checked: sent again on top
          // of a newer one, it could take back someone else's change.
          const latest = undoable && isApiError(error) && error.code === 'version_conflict' ? (error.details as { currentVersion?: unknown } | null)?.currentVersion : null
          if (typeof latest !== 'number') throw error
          return updateRecord(projectId, collectionId, record.id, latest, change)
        }
      }
      const inverse = inverseOf(record, change)
      const undo: Undoable | null = undoable
        ? {
            label: `Changed ${recordName(record)}`,
            undo: async () => {
              // Once the change itself has saved.
              await inOrder(`record:${record.id}`, async () => undefined)
              // As the server has it now: this page may not have heard of a
              // change made since.
              const now = (await getRecord(projectId, collectionId, record.id)).record
              if (touched(now, change) !== saved) throw new Error(`${recordName(record)} changed since, so it wasn’t undone.`)
              if (!(await saveRef.current?.(now, inverse, undefined, false))) throw new Error(`${recordName(record)} didn’t change back.`)
            },
          }
        : null
      let saved = ''
      if (undo) remember(undo)
      return inOrder(`record:${record.id}`, send)
        .then((body) => {
          settled()
          versions.current.set(record.id, Math.max(body.record.version, versions.current.get(record.id) ?? 0))
          // The group it's listed under stays until the list refetches. A save
          // answers without history, so an open record keeps its own; the
          // change's entry comes with the refetch collection.updated starts.
          // An answer older than what already loaded (someone changed the
          // record again meanwhile) never replaces it: that loads again, without
          // this change shown on top.
          let overtaken = false
          patch(
            record.id,
            (current) => {
              if (current.version <= body.record.version) return withUnsaved({ ...body.record, group: current.group })
              overtaken = true
              return current
            },
            body.references,
          )
          if (overtaken || change.archived !== undefined) void refresh()
          saved = touched(body.record, change)
          return true
        })
        .catch((error: unknown) => {
          settled()
          if (undo) forget(undo)
          toastError(`${record.title || `#${record.number}`} didn’t change`, error)
          void refresh()
          return false
        })
    },
    [collectionId, patch, projectId, refresh, withUnsaved],
  )
  useEffect(() => {
    saveRef.current = save
  }, [save])

  /** Adds a record, then refetches the lists so it shows where the view sorts it. */
  const add = useCallback(
    async (title: string, values?: Record<string, FieldValue>) => {
      try {
        const body = await createRecord(projectId, collectionId, { title, values })
        void refresh()
        return body.record
      } catch (error) {
        toastError('The record wasn’t added', error)
        return null
      }
    },
    [collectionId, projectId, refresh],
  )

  return { save, add }
}

export type RecordEdits = ReturnType<typeof useRecordEdits>
