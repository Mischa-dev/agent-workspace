import { useState, type DragEvent } from 'react'

import { toastError } from '../app/toastError'
import { useWorkspaceHref } from '../app/workspace'
import { orderAfterDrop } from '../shell/dragOrder'
import { SidebarLink, SidebarSection } from '../shell/SidebarParts'
import type { IconName, MenuEntry } from '../ui'
import { usePrefetchTimeline } from './prefetchTimeline'
import { SectionNameDialog } from './SectionNameDialog'
import { sidebarItemHref, useSidebarSections, useSidebarWrites, type SidebarItem, type SidebarSection as Section } from './sidebarSections'

const SORTS = [
  { sort: 'manual', label: 'As placed' },
  { sort: 'alpha', label: 'By name' },
  { sort: 'recent', label: 'Newest activity first' },
] as const

type Drag = { id: string; from: string; over: { id: string; section: string; after: boolean } | null }
type Naming = { section: Section | null }

function itemIcon(item: SidebarItem): IconName {
  if (item.kind === 'channel') return item.private ? 'lock' : 'hash'
  return item.kind === 'group' ? 'users' : 'message'
}

/**
 * Your own sidebar sections (C2.9) at the top of a sidebar: Starred, then the
 * ones you named, each folding, sorting and showing only unread as you set
 * it. In a section kept as placed, drag a row to move it, also into another
 * section.
 */
export default function SidebarSections({ onNavigate }: { onNavigate: () => void }) {
  const sidebar = useSidebarSections()
  const writes = useSidebarWrites()
  const prefetch = usePrefetchTimeline()
  const href = useWorkspaceHref()
  const [drag, setDrag] = useState<Drag | null>(null)
  const [naming, setNaming] = useState<Naming | null>(null)
  const sections = sidebar.data?.sections ?? []
  if (sections.length === 0 && !naming) return null

  const fail = (title: string) => (reason: unknown) => toastError(title, reason)
  const change = (section: Section, input: Omit<Parameters<typeof writes.setSection>[0], 'sectionId'>) =>
    void writes.setSection({ sectionId: section.id, ...input }).catch(fail('Your sidebar didn’t change'))

  const menuFor = (section: Section): MenuEntry[] => [
    { heading: 'Sort' },
    ...SORTS.map((option) => ({
      label: option.label,
      checked: section.sort === option.sort,
      onSelect: () => change(section, { sort: option.sort }),
    })),
    { separator: true as const },
    { label: 'Only unread', checked: section.show === 'unread', onSelect: () => change(section, { show: section.show === 'unread' ? 'all' : 'unread' }) },
    { separator: true as const },
    { label: 'New section', icon: 'plus' as const, onSelect: () => setNaming({ section: null }) },
    ...(section.kind === 'custom' ? [{ label: 'Rename', icon: 'pencil' as const, onSelect: () => setNaming({ section }) }] : []),
    {
      label: 'Remove section',
      icon: 'trash' as const,
      danger: true,
      // What was in it goes back where it was, so nothing is lost.
      onSelect: () => change(section, { delete: true }),
    },
  ]

  const dragOver = (event: DragEvent<HTMLLIElement>, section: Section, item: SidebarItem) => {
    if (!drag || section.sort !== 'manual') return
    event.preventDefault()
    const box = event.currentTarget.getBoundingClientRect()
    const after = event.clientY > box.top + box.height / 2
    if (drag.over?.id !== item.conversationId || drag.over.after !== after) {
      setDrag({ ...drag, over: { id: item.conversationId, section: section.id, after } })
    }
  }
  const drop = (event: DragEvent<HTMLLIElement>, section: Section) => {
    event.preventDefault()
    const current = drag
    setDrag(null)
    if (!current?.over) return
    const target = current.over
    const dragged = sections.flatMap((each) => each.items).find((item) => item.conversationId === current.id)
    if (!dragged) return
    // Within a section the list's new order says where; from another, it lands beside the target.
    const list = (current.from === section.id ? section.items : [...section.items, dragged]).map((item) => ({ id: item.conversationId }))
    const order = orderAfterDrop(list, current.id, { id: target.id, after: target.after })
    if (!order) return
    void writes
      .moveItem({ conversationId: current.id, sectionId: section.id, position: order.findIndex((item) => item.id === current.id) })
      .catch(fail('It didn’t move'))
  }

  return (
    <>
      {sections.map((section) => (
        <SidebarSection
          key={section.id}
          label={section.name}
          menu={menuFor(section)}
          onToggle={(open) => change(section, { collapsed: !open })}
          open={!section.collapsed}
        >
          {section.items.filter((item) => !item.hidden && (section.show !== 'unread' || item.unread)).map((item) => {
            const mark = drag?.over?.id === item.conversationId && drag.id !== item.conversationId ? (drag.over.after ? 'is-drop-after' : 'is-drop-before') : undefined
            const movable = section.sort === 'manual'
            return (
              <SidebarLink
                icon={itemIcon(item)}
                keep={item.unread}
                key={item.conversationId}
                label={item.name}
                onIntent={item.kind === 'channel' ? () => prefetch(item.spaceId, item.conversationId) : undefined}
                onNavigate={onNavigate}
                row={{
                  className: ['aw-sidebar__placed', mark, drag?.id === item.conversationId ? 'is-dragging' : undefined].filter(Boolean).join(' '),
                  draggable: movable,
                  onDragEnd: () => setDrag(null),
                  onDragOver: (event) => dragOver(event, section, item),
                  onDragStart: (event) => {
                    event.dataTransfer.effectAllowed = 'move'
                    setDrag({ id: item.conversationId, from: section.id, over: null })
                  },
                  onDrop: (event) => drop(event, section),
                }}
                to={sidebarItemHref(item, href)}
                unread={item.unread}
              />
            )
          })}
        </SidebarSection>
      ))}
      {naming ? (
        <SectionNameDialog
          initial={naming.section?.name}
          onClose={() => setNaming(null)}
          onSave={(name) => writes.setSection(naming.section ? { sectionId: naming.section.id, name } : { name })}
          title={naming.section ? 'Rename section' : 'New section'}
        />
      ) : null}
    </>
  )
}
