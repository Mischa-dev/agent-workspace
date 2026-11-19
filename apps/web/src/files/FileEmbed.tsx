// A file of a page (C1.33), shown where it was put: a picture full width,
// anything else as a card with the start of its text and a download link.
// The file is read with the reader's own access, so someone who can't see
// the page can't see its files either.
import { useQuery } from '@tanstack/react-query'

import { Icon, ListSkeleton } from '../ui'
import { fileContentUrl, formatBytes, getFile, isInlineImage } from './api'
import { fileKeys } from './keys'
import { Attachments } from './Attachments'
import '../collections/collectionEmbed.css'
import './files.css'

export default function FileEmbed({ fileId, label }: { fileId: string; label: string }) {
  const file = useQuery({ queryKey: fileKeys.file(fileId), queryFn: () => getFile(fileId), staleTime: 5 * 60_000 })
  if (file.isPending) return <ListSkeleton label={`Loading ${label || 'the file'}`} rows={2} />
  if (file.isError || !file.data) {
    return (
      <p className="aw-cembed__problem">
        <Icon name="circle-alert" size={16} />
        <span>{label || 'This file'} can’t be shown: it was removed, or you can’t see it.</span>
      </p>
    )
  }
  const found = file.data
  if (isInlineImage(found)) {
    return (
      <figure className="aw-file-embed">
        <a href={fileContentUrl(found.id)} rel="noreferrer" target="_blank">
          <img alt={label || found.name} decoding="async" height={found.height ?? undefined} loading="lazy" src={fileContentUrl(found.id)} width={found.width ?? undefined} />
        </a>
        {label && label !== found.name ? <figcaption>{label}</figcaption> : null}
      </figure>
    )
  }
  return (
    <div className="aw-file-embed is-file" title={`${found.name}, ${formatBytes(found.size)}`}>
      <Attachments files={[found]} />
    </div>
  )
}
