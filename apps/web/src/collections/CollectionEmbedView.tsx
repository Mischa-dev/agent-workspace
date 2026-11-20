import type { EmbedViewProps } from '../modules/define'
import CollectionEmbed from './CollectionEmbed'

/** A collection's view in a page, from what its embed's path says (M1.5). */
export default function CollectionEmbedView({ label, params }: EmbedViewProps) {
  return <CollectionEmbed collectionId={params.collectionId ?? ''} label={label} projectId={params.projectId ?? ''} viewId={params.viewId ?? null} />
}
