import { useState } from 'react'

import { toastError } from '../app/toastError'
import { showToast, type MenuEntry } from '../ui'
import { SectionNameDialog } from './SectionNameDialog'
import { sectionOf, useSidebarSections, useSidebarWrites } from './sidebarSections'

/**
 * A channel's, DM's or group's place in your sidebar sections (C2.9), for its
 * header menu: star it, move it into a section you named or a new one, or
 * take it out. `shown` is the layout's say (`talk.sidebar_sections`).
 */
export function useSidebarPlacement(conversationId: string, shown: boolean) {
  const sidebar = useSidebarSections(shown && Boolean(conversationId))
  const writes = useSidebarWrites()
  const [naming, setNaming] = useState(false)
  if (!shown || !conversationId) return { menu: [] as MenuEntry[], dialog: null }

  const sections = sidebar.data?.sections ?? []
  const here = sectionOf(sidebar.data, conversationId)
  const starred = here?.kind === 'starred'
  const move = (input: { sectionId?: string; starred?: boolean }, done: string) =>
    writes
      .moveItem({ conversationId, ...input })
      .then(() => showToast({ tone: 'success', title: done }))
      .catch((reason: unknown) => toastError('Your sidebar didn’t change', reason))

  const menu: MenuEntry[] = [
    { heading: 'Sidebar' },
    {
      label: starred ? 'Unstar' : 'Star',
      icon: 'star',
      onSelect: () => void move({ starred: !starred }, starred ? 'Unstarred' : 'Starred'),
    },
    ...sections
      .filter((section) => section.kind === 'custom')
      .map((section) => ({
        label: `In ${section.name}`,
        checked: here?.id === section.id,
        onSelect: () =>
          void (here?.id === section.id ? move({}, `Taken out of ${section.name}`) : move({ sectionId: section.id }, `Moved to ${section.name}`)),
      })),
    { label: 'New section…', icon: 'plus', onSelect: () => setNaming(true) },
  ]

  const dialog = naming ? (
    <SectionNameDialog
      onClose={() => setNaming(false)}
      onSave={async (name) => {
        const made = await writes.setSection({ name })
        const section = made.sections.find((candidate) => candidate.name === name)
        if (section) await move({ sectionId: section.id }, `Moved to ${name}`)
      }}
      title="New section"
    />
  ) : null

  return { menu, dialog }
}
