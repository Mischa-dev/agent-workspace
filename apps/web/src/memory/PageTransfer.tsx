import { useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Download, FileUp } from 'lucide-react'
import { useNavigate } from 'react-router-dom'

import { toastError } from '../app/mutations'
import { Button, Menu, showToast } from '../ui'
import { PAGE_IMPORT_ACCEPT, importPage, memoryKeys, pageExportHref } from './api'

/** Downloads a page as Markdown or a Word document. */
export function PageDownloadMenu({ pageId }: { pageId: string }) {
  const download = (format: 'markdown' | 'docx') => {
    const link = document.createElement('a')
    link.href = pageExportHref(pageId, format)
    link.download = ''
    link.rel = 'noopener'
    document.body.append(link)
    link.click()
    link.remove()
  }
  return (
    <Menu
      items={[
        { label: 'Markdown (.md)', onSelect: () => download('markdown') },
        { label: 'Word document (.docx)', onSelect: () => download('docx') },
      ]}
      label="Download page"
      placement="bottom-end"
      trigger={<Button icon={Download}>Download</Button>}
    />
  )
}

/** Makes a page from a Word document or Markdown file, then opens it. */
export function PageImportButton({ pagesHref, projectId }: { pagesHref: string; projectId: string }) {
  const input = useRef<HTMLInputElement>(null)
  const [busy, setBusy] = useState(false)
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const choose = (file: File | undefined) => {
    if (!file) return
    setBusy(true)
    void importPage(projectId, file)
      .then(async ({ page, setAside }) => {
        await queryClient.invalidateQueries({ queryKey: memoryKeys.pages(projectId) })
        showToast({
          title: `“${page.title}” imported`,
          description: setAside.length ? `Left out: ${setAside.join(', ')}. A page can’t hold them.` : undefined,
        })
        navigate(`${pagesHref}/${encodeURIComponent(page.id)}`)
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
        accept={PAGE_IMPORT_ACCEPT}
        aria-label="Import a Word document or Markdown file"
        hidden
        onChange={(event) => choose(event.target.files?.[0])}
        ref={input}
        type="file"
      />
    </>
  )
}
