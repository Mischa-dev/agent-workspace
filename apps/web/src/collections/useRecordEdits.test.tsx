import { act, renderHook, waitFor } from '@testing-library/react'
import { QueryClient, QueryClientProvider, type InfiniteData } from '@tanstack/react-query'
import type { ReactNode } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { collectionKeys, updateRecord, type CollectionRecord, type RecordBody, type RecordPage } from './api'
import { useRecordEdits } from './useRecordEdits'

vi.mock('./api', async (importOriginal) => ({ ...(await importOriginal<typeof import('./api')>()), updateRecord: vi.fn() }))
vi.mock('../app/toastError', () => ({ toastError: vi.fn() }))

const person = { id: 'person-a', name: 'Ada' }
const record: CollectionRecord = {
  id: 'record-a',
  number: 1,
  title: 'Floaty jump',
  values: {},
  createdBy: person,
  updatedBy: person,
  archived: false,
  version: 1,
  createdAt: '',
  updatedAt: '',
  group: null,
}
const recordsKey = collectionKeys.records('project-a', 'collection-a', 'view=a')
const recordKey = collectionKeys.record('project-a', 'collection-a', record.id)
const page = (records: CollectionRecord[]): RecordPage => ({ records, references: [], total: records.length, groups: [], nextCursor: null })
const body = (saved: CollectionRecord): RecordBody => ({ record: saved, references: [], history: [] })

/** A save the test answers when it chooses. */
function answers() {
  const pending: Array<(body: RecordBody) => void> = []
  vi.mocked(updateRecord).mockImplementation(() => new Promise((resolve) => pending.push(resolve)))
  return pending
}

function setup() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  client.setQueryData<InfiniteData<RecordPage>>(recordsKey, { pages: [page([record])], pageParams: [null] })
  client.setQueryData<RecordBody>(recordKey, body(record))
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>
  const listed = () => client.getQueryData<InfiniteData<RecordPage>>(recordsKey)?.pages[0].records[0]
  const opened = () => client.getQueryData<RecordBody>(recordKey)?.record
  return { client, listed, opened, ...renderHook(() => useRecordEdits('project-a', 'collection-a'), { wrapper }) }
}

afterEach(() => vi.clearAllMocks())

describe('useRecordEdits', () => {
  it('keeps showing a later change when the answer to an earlier one comes back', async () => {
    const pending = answers()
    const { result, listed, opened } = setup()

    act(() => {
      void result.current.save(record, { values: { platform: 'PC' } })
      void result.current.save(record, { values: { tags: ['camera', 'jump'] } })
    })
    expect(listed()?.values).toEqual({ platform: 'PC', tags: ['camera', 'jump'] })

    await waitFor(() => expect(updateRecord).toHaveBeenCalledTimes(1))
    await act(async () => pending[0](body({ ...record, values: { platform: 'PC' }, version: 2 })))
    expect(listed()).toMatchObject({ values: { platform: 'PC', tags: ['camera', 'jump'] }, version: 2 })
    expect(opened()?.values).toEqual({ platform: 'PC', tags: ['camera', 'jump'] })

    // The second goes against the version the first left.
    await waitFor(() => expect(updateRecord).toHaveBeenCalledTimes(2))
    expect(updateRecord).toHaveBeenLastCalledWith('project-a', 'collection-a', record.id, 2, { values: { tags: ['camera', 'jump'] } })
    await act(async () => pending[1](body({ ...record, values: { platform: 'PC', tags: ['camera', 'jump'] }, version: 3 })))
    expect(listed()).toMatchObject({ values: { platform: 'PC', tags: ['camera', 'jump'] }, version: 3 })
  })

  it('keeps the open record’s history when a save answers without it', async () => {
    const pending = answers()
    const { client, result } = setup()
    const added = { id: 'entry-a', action: 'created', actor: 'Ada', via: null, changes: [], version: 1, at: '' }
    client.setQueryData<RecordBody>(recordKey, { ...body(record), history: [added] })

    act(() => void result.current.save(record, { values: { platform: 'PC' } }))
    await waitFor(() => expect(updateRecord).toHaveBeenCalledTimes(1))
    await act(async () => pending[0](body({ ...record, values: { platform: 'PC' }, version: 2 })))
    expect(client.getQueryData<RecordBody>(recordKey)).toMatchObject({ record: { values: { platform: 'PC' }, version: 2 }, history: [added] })
  })

  it('keeps a newer version that loaded before the answer, and loads it again', async () => {
    const pending = answers()
    const { client, result, listed, opened } = setup()
    const invalidate = vi.spyOn(client, 'invalidateQueries')

    act(() => void result.current.save(record, { title: 'Floaty double jump' }))
    await waitFor(() => expect(updateRecord).toHaveBeenCalledTimes(1))
    // Ours saved as version 2, then someone else's change made version 3, which loads first.
    const newer = { ...record, title: 'Floaty double jump', values: { platform: 'Mac' }, version: 3 }
    await act(async () => {
      await client.fetchInfiniteQuery({ queryKey: recordsKey, queryFn: async () => page([newer]), initialPageParam: null })
      await client.fetchQuery({ queryKey: recordKey, queryFn: async () => body(newer) })
    })
    await act(async () => pending[0](body({ ...record, title: 'Floaty double jump', version: 2 })))

    expect(listed()).toMatchObject({ values: { platform: 'Mac' }, version: 3 })
    expect(opened()).toMatchObject({ values: { platform: 'Mac' }, version: 3 })
    expect(invalidate).toHaveBeenCalledWith({ queryKey: collectionKeys.one('project-a', 'collection-a') })
  })

  it('shows changes still saving on top of a list or record that loads meanwhile', async () => {
    const pending = answers()
    const { client, result, listed, opened } = setup()

    act(() => void result.current.save(record, { title: 'Floaty double jump' }))
    await waitFor(() => expect(updateRecord).toHaveBeenCalledTimes(1))
    // Someone else's change arrives in a refetch before ours is saved.
    const theirs = { ...record, values: { platform: 'Mac' }, version: 2 }
    await act(async () => {
      await client.fetchInfiniteQuery({ queryKey: recordsKey, queryFn: async () => page([theirs]), initialPageParam: null })
      await client.fetchQuery({ queryKey: recordKey, queryFn: async () => body(theirs) })
    })
    expect(listed()).toMatchObject({ title: 'Floaty double jump', values: { platform: 'Mac' } })
    expect(opened()).toMatchObject({ title: 'Floaty double jump', values: { platform: 'Mac' } })

    await act(async () => pending[0](body({ ...theirs, title: 'Floaty double jump', version: 3 })))
    // Once saved, what loads next is what the server says.
    await act(async () => {
      await client.fetchQuery({ queryKey: recordKey, queryFn: async () => body({ ...theirs, title: 'Renamed again', version: 4 }) })
    })
    expect(opened()?.title).toBe('Renamed again')
  })
})
