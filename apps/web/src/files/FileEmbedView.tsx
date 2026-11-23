import type { EmbedViewProps } from '../modules/define'
import FileEmbed from './FileEmbed'

/** A file of the page (C1.33) as the pages module registers it (W1.26). */
export default function FileEmbedView({ label, params }: EmbedViewProps) {
  return <FileEmbed fileId={params.fileId ?? ''} label={label} />
}
