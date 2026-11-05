import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'

import { toastError } from '../app/mutations'
import { Section, Switch, showToast } from '../ui'
import { layoutKeys, refreshLayouts, saveMyView, type MyView } from './api'
import type { EffectiveLayout } from './layout'
import { PartSwitches, SidebarPreview } from './LayoutEditor'
import type { PartId } from './parts'
import { REGISTRY, everything } from './registry'

const SOURCES: Record<EffectiveLayout['source'], string> = {
  person: 'An owner or admin chose it for you by name.',
  role: 'It’s the layout for your role here.',
  project: 'It’s this project’s layout.',
  workspace: 'It’s the workspace’s default.',
  default: 'No layout is chosen, so everything shows.',
}

/** For people who can't change the layout: what they see, and why. */
export function YourLayout({ effective, previewName }: { effective: EffectiveLayout; previewName: string }) {
  const visible = new Set(REGISTRY.parts.map((part) => part.id).filter((part) => !effective.layoutHidden.includes(part)))
  return (
    <div className="aw-layout__editor">
      <div className="aw-layout__main">
        <div className="aw-layoutsummary">
          <h2 className="aw-layoutsummary__title">You see {effective.preset?.name ?? 'everything'}</h2>
          <p className="aw-layoutsummary__text">
            {SOURCES[effective.source]} Owners and admins choose layouts. You can hide more for yourself below.
          </p>
        </div>
      </div>
      <aside className="aw-layout__aside">
        <SidebarPreview projectName={previewName} shape={{ ...everything(), visible, words: effective.words, lead: effective.lead }} />
      </aside>
    </div>
  )
}

/** For owners and admins whose own layout comes from a rule: the editor below is for everyone else. */
export function OwnRuleNote({ effective }: { effective: EffectiveLayout }) {
  if (effective.source !== 'person' && effective.source !== 'role') return null
  return (
    <p className="aw-layoutsummary__text">
      You see {effective.preset?.name ?? 'everything'}, {effective.source === 'person' ? 'chosen for you by name' : 'chosen for your role'}. The layout below is for everyone without a rule.
    </p>
  )
}

/**
 * "My view": hide more for yourself, across the workspace, and (for owners
 * and admins) show what layouts hide. It saves as you switch.
 */
export function MyViewSection({ layoutHidden, myView, organizationId }: { layoutHidden: string[]; myView: MyView; organizationId: string }) {
  const queryClient = useQueryClient()
  const [hidden, setHidden] = useState(() => new Set(myView.hidden))
  const [showHidden, setShowHidden] = useState(myView.showHidden)
  const save = useMutation({
    mutationFn: (next: { hidden: Set<string>; showHidden: boolean }) => saveMyView(organizationId, [...next.hidden], next.showHidden),
    onSuccess: (response) => {
      queryClient.setQueryData(layoutKeys.workspace(organizationId), response)
      void refreshLayouts(queryClient)
    },
    onError: (error) => {
      setHidden(new Set(myView.hidden))
      setShowHidden(myView.showHidden)
      toastError('Your view didn’t change', error)
    },
  })
  const change = (next: { hidden: Set<string>; showHidden: boolean }) => {
    setHidden(next.hidden)
    setShowHidden(next.showHidden)
    save.mutate(next)
  }
  const shown = new Set(REGISTRY.parts.map((part) => part.id).filter((part) => !hidden.has(part)))
  // What the layout hides can't be hidden again, unless you're seeing it anyway.
  const locked = showHidden && myView.canShowHidden ? undefined : { parts: new Set(layoutHidden), why: 'The layout already hides it.' }
  return (
    <Section
      description="Hide more for yourself, in every project of this workspace. Your view only ever hides; the layout decides the rest."
      id="layout-my-view"
      title="Your view"
    >
      {myView.canShowHidden ? (
        <Switch
          checked={showHidden}
          description="See every part layouts hide, for you only, so nothing is out of reach."
          label="Show hidden parts"
          onChange={(checked) => {
            change({ hidden, showHidden: checked })
            showToast({ tone: 'success', title: checked ? 'Showing hidden parts' : 'Hidden parts hidden again' })
          }}
        />
      ) : null}
      <details className="aw-layout__mine">
        <summary>{hidden.size ? `Parts you hid (${hidden.size})` : 'Hide parts for yourself'}</summary>
        <PartSwitches
          disabled={save.isPending}
          lockedOff={locked}
          onToggle={(part: PartId, visible) => {
            const next = new Set(hidden)
            if (visible) next.delete(part)
            else next.add(part)
            change({ hidden: next, showHidden })
          }}
          visible={shown}
        />
      </details>
    </Section>
  )
}
