import type { EmbedViewProps } from '../modules/define'
import SheetEmbed from './SheetEmbed'

/** A sheet's range in a page, from what its embed's path says (M1.5). */
export default function SheetEmbedView({ label, params }: EmbedViewProps) {
  return (
    <SheetEmbed
      label={label}
      name={params.name ?? null}
      projectId={params.projectId ?? ''}
      range={params.range ?? null}
      sheetId={params.sheetId ?? ''}
      tabId={params.tabId ?? null}
    />
  )
}
