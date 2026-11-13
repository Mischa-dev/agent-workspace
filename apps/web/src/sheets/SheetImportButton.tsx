import { useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { FileUp } from 'lucide-react'

import { toastError } from '../app/mutations'
import { Button, showToast } from '../ui'
import { SHEET_IMPORT_ACCEPT, importSheet, sheetKeys, type SheetBody } from './api'

/** Makes a sheet from an Excel (.xlsx) or CSV file, then opens it. */
export function SheetImportButton({ onImported, projectId }: { onImported: (body: SheetBody) => void; projectId: string }) {
  const input = useRef<HTMLInputElement>(null)
  const [busy, setBusy] = useState(false)
  const client = useQueryClient()
  const choose = (file: File | undefined) => {
    if (!file) return
    setBusy(true)
    void importSheet(projectId, file)
      .then(async (body) => {
        await client.invalidateQueries({ queryKey: sheetKeys.list(projectId) })
        showToast({ tone: 'success', title: `“${body.sheet.name}” imported` })
        onImported(body)
      })
      .catch((reason: unknown) => toastError('That file wasn’t imported', reason))
      .finally(() => {
        setBusy(false)
        if (input.current) input.current.value = ''
      })
  }
  return (
    <>
      <Button busy={busy} icon={FileUp} onClick={() => input.current?.click()}>
        Import
      </Button>
      <input
        accept={SHEET_IMPORT_ACCEPT}
        aria-label="Import an Excel or CSV file"
        hidden
        onChange={(event) => choose(event.target.files?.[0])}
        ref={input}
        type="file"
      />
    </>
  )
}
