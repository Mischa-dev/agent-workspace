import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Bookmark, PanelsTopLeft } from 'lucide-react'

import { toastError } from '../../app/mutations'
import { changePreset, createPreset, layoutKeys, refreshLayouts, removePreset, saveWorkspaceLayout, useWorkspaceLayout } from '../../experience/api'
import { LayoutEditor, SavePresetDialog, VisualOnlyNote } from '../../experience/LayoutEditor'
import { MyViewSection, YourLayout } from '../../experience/LayoutSections'
import { presetName, type Layout, type SavedPreset } from '../../experience/registry'
import { ConfirmDialog, EmptyState, ErrorState, Icon, IconButton, Menu, PageHeader, Row, RowList, Section, SettingsSkeleton, showToast } from '../../ui'
import type { SettingsPageProps } from '../types'

/** Workspace › Layout: the default every project follows, the workspace's own presets, and your view. */
export default function LayoutPage({ workspace }: SettingsPageProps) {
  return workspace ? <WorkspaceLayout key={workspace.id} name={workspace.name} organizationId={workspace.id} /> : null
}

function WorkspaceLayout({ name, organizationId }: { name: string; organizationId: string }) {
  const queryClient = useQueryClient()
  const layout = useWorkspaceLayout(organizationId)
  const save = useMutation({
    mutationFn: ({ next, version }: { next: Layout | null; version: number }) => saveWorkspaceLayout(organizationId, next, version),
    onSuccess: (response) => {
      queryClient.setQueryData(layoutKeys.workspace(organizationId), response)
      void refreshLayouts(queryClient)
      showToast({ tone: 'success', title: 'Default saved', description: 'Projects without a layout of their own follow it now.' })
    },
    onError: (error) => {
      if (toastError('The default wasn’t saved', error)) void layout.refetch()
    },
  })

  if (layout.isPending) return <SettingsSkeleton label="Loading the layout" />
  if (layout.isError) return <ErrorState error={layout.error} onRetry={() => void layout.refetch()} title="The layout didn’t load" />
  const view = layout.data
  const manage = view.capabilities.manageWorkspace

  const savePreset = async (presetName: string, description: string, next: Layout) => {
    try {
      await createPreset(organizationId, { name: presetName, description, layout: next })
      await refreshLayouts(queryClient)
      showToast({ tone: 'success', title: `Saved ${presetName}`, description: 'It’s a choice now for every project in the workspace.' })
    } catch (error) {
      toastError('The preset wasn’t saved', error)
      throw error
    }
  }

  return (
    <>
      <PageHeader
        mark={
          <span className="aw-pagehead__icon">
            <Icon name={PanelsTopLeft} size={20} />
          </span>
        }
        title="Layout"
      >
        What projects in {name} show by default, the presets saved here, and what you hide for yourself.
      </PageHeader>
      <VisualOnlyNote>Each project can pick its own layout in its settings, and give roles or people their own preset.</VisualOnlyNote>
      {manage ? (
        <LayoutEditor
          canEdit
          canSavePreset
          cardsLabel="Default for projects"
          fallback={{ name: 'Everything on', description: 'No default: projects show everything until they pick a layout.', layout: null }}
          key={view.version}
          onSave={(next) => save.mutateAsync({ next, version: view.version })}
          onSavePreset={savePreset}
          previewName="A project"
          saved={view.savedPresets}
          stored={view.layout}
        />
      ) : (
        <YourLayout effective={view.effective} previewName="A project" />
      )}
      <SavedPresets canManage={manage} organizationId={organizationId} presets={view.savedPresets.filter((preset) => !preset.removed)} />
      <MyViewSection key={`${view.myView.hidden.join()}:${view.myView.showHidden}`} layoutHidden={view.effective.layoutHidden} myView={view.myView} organizationId={organizationId} />
    </>
  )
}

/** The workspace's own presets: rename them or remove them. Projects using a removed one keep it until they pick another. */
function SavedPresets({ canManage, organizationId, presets }: { canManage: boolean; organizationId: string; presets: SavedPreset[] }) {
  const queryClient = useQueryClient()
  const [renaming, setRenaming] = useState<SavedPreset | null>(null)
  const [removing, setRemoving] = useState<SavedPreset | null>(null)
  const remove = useMutation({
    mutationFn: (preset: SavedPreset) => removePreset(organizationId, preset.id),
    onSuccess: (_, preset) => {
      setRemoving(null)
      showToast({ tone: 'success', title: `${preset.name} removed` })
    },
    onError: (error) => toastError('The preset wasn’t removed', error),
    onSettled: () => refreshLayouts(queryClient),
  })
  return (
    <Section
      description="Presets saved in this workspace show next to the built-in ones in every project. Save one from the editor above."
      id="layout-saved-presets"
      title="Saved presets"
    >
      {presets.length ? (
        <RowList label="Saved presets">
          {presets.map((preset) => (
            <Row
              actions={
                canManage ? (
                  <Menu
                    items={[
                      { label: 'Rename…', icon: 'pencil', onSelect: () => window.requestAnimationFrame(() => setRenaming(preset)) },
                      { separator: true },
                      { label: 'Remove', icon: 'trash', danger: true, onSelect: () => window.requestAnimationFrame(() => setRemoving(preset)) },
                    ]}
                    label={`${preset.name} actions`}
                    trigger={<IconButton icon="more" label={`${preset.name} actions`} size="sm" />}
                  />
                ) : null
              }
              key={preset.id}
              leading={
                <span className="aw-row__tile">
                  <Icon name={Bookmark} size={16} />
                </span>
              }
              meta={[preset.description, `Starts from ${presetName(preset.base, [])}`].filter(Boolean).join(' · ')}
              title={preset.name}
            />
          ))}
        </RowList>
      ) : (
        <EmptyState icon="bookmark" title="No saved presets yet">
          Shape a layout above, then save it as a preset to use it in any project here.
        </EmptyState>
      )}
      {renaming ? (
        <SavePresetDialog
          initial={{ name: renaming.name, description: renaming.description }}
          onClose={() => setRenaming(null)}
          onSave={async (presetName, description) => {
            try {
              await changePreset(organizationId, renaming.id, { name: presetName, description, expectedVersion: renaming.version })
              await refreshLayouts(queryClient)
              setRenaming(null)
              showToast({ tone: 'success', title: 'Preset renamed' })
            } catch (error) {
              toastError('The preset didn’t change', error)
              throw error
            }
          }}
          title="Rename preset"
        />
      ) : null}
      <ConfirmDialog
        busy={remove.isPending}
        confirmLabel="Remove preset"
        onClose={() => setRemoving(null)}
        onConfirm={() => removing && remove.mutate(removing)}
        open={Boolean(removing)}
        title={`Remove ${removing?.name ?? 'this preset'}?`}
      >
        It won’t be offered any more. Projects, roles and people using it keep it until someone picks another.
      </ConfirmDialog>
    </Section>
  )
}
