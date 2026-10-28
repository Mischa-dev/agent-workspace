import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { PanelsTopLeft } from 'lucide-react'

import { queryKeys } from '../../api/queryKeys'
import { toastError } from '../../app/mutations'
import { createPreset, layoutKeys, refreshLayouts, saveProjectLayout, saveProjectRules, useProjectLayout, type LayoutRule, type ProjectLayoutView, type RuleRole } from '../../experience/api'
import { LayoutEditor, VisualOnlyNote } from '../../experience/LayoutEditor'
import { MyViewSection, OwnRuleNote, YourLayout } from '../../experience/LayoutSections'
import { REGISTRY, presetName, type Layout, type SavedPreset } from '../../experience/registry'
import type { Project } from '../../projects/types'
import { listMembers } from '../../team/api'
import { Button, ErrorState, Icon, IconButton, PageHeader, Section, SelectField, SettingsSkeleton, showToast } from '../../ui'
import type { SettingsPageProps } from '../types'

/** Project › Layout: what the project shows, for everyone, for each role and for named people. */
export default function LayoutPage({ project }: SettingsPageProps) {
  return project ? <ProjectLayout key={project.id} project={project} /> : null
}

function ProjectLayout({ project }: { project: Project }) {
  const queryClient = useQueryClient()
  const layout = useProjectLayout(project.id)
  const save = useMutation({
    mutationFn: ({ next, version }: { next: Layout | null; version: number }) => saveProjectLayout(project.id, next, version),
    onSuccess: (response) => {
      queryClient.setQueryData(layoutKeys.project(project.id), response)
      showToast({ tone: 'success', title: 'Layout saved', description: 'Everyone in the project sees it now.' })
    },
    onError: (error) => {
      if (toastError('The layout wasn’t saved', error)) void layout.refetch()
    },
  })

  if (layout.isPending) return <SettingsSkeleton label="Loading the layout" />
  if (layout.isError) return <ErrorState error={layout.error} onRetry={() => void layout.refetch()} title="The layout didn’t load" />
  const view = layout.data
  const manage = view.capabilities.manage && !project.archivedAt

  const savePreset = async (name: string, description: string, next: Layout) => {
    try {
      await createPreset(view.organizationId, { name, description, layout: next })
      await refreshLayouts(queryClient)
      showToast({ tone: 'success', title: `Saved ${name}`, description: 'Pick it above to use it here, or in any project of the workspace.' })
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
        What {project.name} shows and what it calls things, for everyone, for each role and for named people.
      </PageHeader>
      <VisualOnlyNote>{view.myView.canShowHidden ? 'Owners and admins can show hidden parts for themselves under Your view.' : null}</VisualOnlyNote>
      {manage ? (
        <>
          <OwnRuleNote effective={view.effective} />
          <LayoutEditor
            canEdit
            canSavePreset={view.capabilities.manageWorkspace}
            cardsLabel="What’s this project for?"
            fallback={{
              name: 'Workspace default',
              description: view.workspaceLayout
                ? `Follow the workspace, which uses ${presetName(view.workspaceLayout.preset, view.savedPresets)} now.`
                : 'Follow the workspace. It has no default, so everything shows.',
              layout: view.workspaceLayout,
            }}
            key={view.version}
            onSave={(next) => save.mutateAsync({ next, version: view.version })}
            onSavePreset={savePreset}
            previewName={project.name}
            saved={view.savedPresets}
            stored={view.layout}
          />
          <Rules project={project} view={view} />
        </>
      ) : (
        <YourLayout effective={view.effective} previewName={project.name} />
      )}
      <MyViewSection key={`${view.myView.hidden.join()}:${view.myView.showHidden}`} layoutHidden={view.effective.layoutHidden} myView={view.myView} organizationId={view.organizationId} />
    </>
  )
}

const ROLES: { role: RuleRole; label: string; meta: string }[] = [
  { role: 'admin', label: 'Owners and admins', meta: 'Everyone who manages the project' },
  { role: 'member', label: 'Members', meta: 'Everyone who works in it' },
  { role: 'viewer', label: 'Viewers', meta: 'Everyone who only reads' },
]

type RuleInput = { role?: RuleRole; actorId?: string; preset: string }

const inputOf = (rule: LayoutRule): RuleInput =>
  rule.person ? { actorId: rule.person.actorId, preset: rule.preset } : { role: rule.role ?? 'member', preset: rule.preset }

function PresetOptions({ current, saved }: { current: string; saved: SavedPreset[] }) {
  return (
    <>
      {REGISTRY.presets.map((preset) => (
        <option key={preset.id} value={preset.id}>
          {preset.name}
        </option>
      ))}
      {saved
        .filter((preset) => !preset.removed || preset.id === current)
        .map((preset) => (
          <option key={preset.id} value={preset.id}>
            {preset.name}
            {preset.removed ? ' (removed)' : ''}
          </option>
        ))}
    </>
  )
}

/** "For roles and people": a preset for each role, or for one person by name, over the project's layout. */
function Rules({ project, view }: { project: Project; view: ProjectLayoutView }) {
  const queryClient = useQueryClient()
  const members = useQuery({ queryKey: queryKeys.members(project.id), queryFn: () => listMembers(project.id) })
  const [person, setPerson] = useState('')
  const [personPreset, setPersonPreset] = useState('bug_reporting')
  const save = useMutation({
    mutationFn: (rules: RuleInput[]) => saveProjectRules(project.id, rules),
    onSuccess: (response) => queryClient.setQueryData(layoutKeys.project(project.id), response),
    onError: (error) => {
      toastError('The rule didn’t change', error)
      void queryClient.invalidateQueries({ queryKey: layoutKeys.project(project.id) })
    },
  })
  const rules = view.rules.map(inputOf)
  const roleRule = (role: RuleRole) => view.rules.find((rule) => rule.role === role)
  const people = view.rules.filter((rule) => rule.person)
  const setRole = (role: RuleRole, preset: string) => {
    const others = rules.filter((rule) => rule.role !== role)
    save.mutate(preset ? [...others, { role, preset }] : others, {
      onSuccess: () => showToast({ tone: 'success', title: preset ? `${ROLES.find((entry) => entry.role === role)?.label} now see ${presetName(preset, view.savedPresets)}` : 'Rule removed' }),
    })
  }
  const setPersonRule = (actorId: string, preset: string | null) => {
    const others = rules.filter((rule) => rule.actorId !== actorId)
    save.mutate(preset ? [...others, { actorId, preset }] : others)
  }
  const candidates = (members.data?.members ?? []).filter(
    (member) => member.enabled && (member.actorKind ?? 'human') === 'human' && !people.some((rule) => rule.person?.actorId === member.actorId),
  )

  return (
    <Section
      description="Give a role, or one person, their own preset. A rule for a person beats one for their role, which beats the project’s layout."
      id="layout-rules"
      title="For roles and people"
    >
      <div className="aw-layoutrules">
        {ROLES.map((entry) => (
          <div className="aw-layoutrule" key={entry.role}>
            <span className="aw-layoutrule__who">
              <span className="aw-layoutrule__name">{entry.label}</span>
              <span className="aw-layoutrule__meta">{entry.meta}</span>
            </span>
            <SelectField
              aria-label={`Preset for ${entry.label.toLocaleLowerCase()}`}
              compact
              disabled={save.isPending}
              onChange={(event) => setRole(entry.role, event.target.value)}
              value={roleRule(entry.role)?.preset ?? ''}
            >
              <option value="">The project’s layout</option>
              <PresetOptions current={roleRule(entry.role)?.preset ?? ''} saved={view.savedPresets} />
            </SelectField>
            <span />
          </div>
        ))}
        {people.map((rule) => (
          <div className="aw-layoutrule" key={rule.id}>
            <span className="aw-layoutrule__who">
              <span className="aw-layoutrule__name">{rule.person?.name}</span>
              <span className="aw-layoutrule__meta">{rule.person?.inProject ? 'By name' : 'Not in the project now'}</span>
            </span>
            <SelectField
              aria-label={`Preset for ${rule.person?.name}`}
              compact
              disabled={save.isPending}
              onChange={(event) => setPersonRule(rule.person?.actorId ?? '', event.target.value)}
              value={rule.preset}
            >
              <PresetOptions current={rule.preset} saved={view.savedPresets} />
            </SelectField>
            <IconButton
              disabled={save.isPending}
              icon="x"
              label={`Remove the rule for ${rule.person?.name}`}
              onClick={() => setPersonRule(rule.person?.actorId ?? '', null)}
              size="sm"
            />
          </div>
        ))}
        {candidates.length ? (
          <form
            className="aw-layoutrule"
            onSubmit={(event) => {
              event.preventDefault()
              if (!person) return
              setPersonRule(person, personPreset)
              setPerson('')
            }}
          >
            <SelectField aria-label="Person" compact onChange={(event) => setPerson(event.target.value)} value={person}>
              <option value="">Add a person…</option>
              {candidates.map((member) => (
                <option key={member.actorId} value={member.actorId}>
                  {member.displayName}
                </option>
              ))}
            </SelectField>
            <SelectField aria-label="Their preset" compact onChange={(event) => setPersonPreset(event.target.value)} value={personPreset}>
              <PresetOptions current={personPreset} saved={view.savedPresets} />
            </SelectField>
            <Button busy={save.isPending} disabled={!person} size="sm" type="submit">
              Add
            </Button>
          </form>
        ) : null}
      </div>
    </Section>
  )
}
