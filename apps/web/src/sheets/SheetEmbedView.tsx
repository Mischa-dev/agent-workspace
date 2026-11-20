import type { EmbedViewProps } from '../modules/define'
import SheetChartEmbed from './SheetChartEmbed'
import SheetEmbed from './SheetEmbed'

/** A sheet's range or one of its charts in a page, from what its embed's path says (M1.5, C1.18). */
export default function SheetEmbedView({ label, params }: EmbedViewProps) {
  if (params.chart) {
    return <SheetChartEmbed chartId={params.chart} label={label} projectId={params.projectId ?? ''} sheetId={params.sheetId ?? ''} />
  }
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
