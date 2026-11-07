import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Blocks } from 'lucide-react'

import { requestJson } from '../../api/client'
import { toastError } from '../../app/mutations'
import { refreshLayouts } from '../../experience/api'
import { Button, ErrorState, Icon, PageHeader, Row, RowList, Section, SettingsSkeleton, StatusChip, Switch, showToast } from '../../ui'
import type { SettingsPageProps } from '../types'

// AWD-75: Workspace › Modules. Which whole features are on in the workspace.
// Off hides a module everywhere (its pages, sidebar rows, palette entries,
// agents' tools, search and notifications) without deleting anything.

type ModuleView = {
  id: string
  name: string
  description: string
  switchable: boolean
  on: boolean
  switchedOn: boolean
  offBecause?: string
  needs: string[]
}

type ModulePreset = { id: string; name: string; description: string; on: string[] }

type ModulesResponse = {
  organizationId: string
  modules: ModuleView[]
  presets: ModulePreset[]
  preset: string | null
  canManage: boolean
}

const modulesKey = (organizationId: string) => ['organization', organizationId, 'modules'] as const
const modulesPath = (organizationId: string) => `/api/v1/organizations/${encodeURIComponent(organizationId)}/modules`

export default function ModulesPage({ workspace }: SettingsPageProps) {
  return workspace ? <WorkspaceModules key={workspace.id} name={workspace.name} organizationId={workspace.id} /> : null
}

function WorkspaceModules({ name, organizationId }: { name: string; organizationId: string }) {
  const queryClient = useQueryClient()
  const modules = useQuery({
    queryKey: modulesKey(organizationId),
    queryFn: () => requestJson<ModulesResponse>(modulesPath(organizationId)),
  })
  const change = useMutation({
    mutationFn: (body: { preset?: string; switches?: Record<string, boolean> }) =>
      requestJson<ModulesResponse>(modulesPath(organizationId), { method: 'PUT', json: body }),
    onSuccess: (response) => {
      queryClient.setQueryData(modulesKey(organizationId), response)
      // What everyone sees follows from the layouts, which carry the modules,
      // and the rail and the command menu from each project's modulesOff.
      void refreshLayouts(queryClient)
      void queryClient.invalidateQueries({ queryKey: ['projects'] })
      showToast({ tone: 'success', title: 'Modules saved', description: 'Everyone in the workspace sees the change.' })
    },
    onError: (error) => {
      if (toastError('The modules weren’t saved', error)) void modules.refetch()
    },
  })

  if (modules.isPending) return <SettingsSkeleton label="Loading modules" />
  if (modules.isError) return <ErrorState error={modules.error} onRetry={() => void modules.refetch()} title="Modules didn’t load" />
  const view = modules.data
  const manage = view.canManage
  const names = new Map(view.modules.map((module) => [module.id, module.name]))

  return (
    <>
      <PageHeader
        mark={
          <span className="aw-pagehead__icon">
            <Icon name={Blocks} size={20} />
          </span>
        }
        title="Modules"
      >
        Which features {name} uses. A module that’s off is hidden everywhere, for people and agents, and nothing in it is
        deleted: turn it back on and everything comes back.
      </PageHeader>
      <Section description="Start from one of these, then switch single modules below." title="Presets">
        <RowList>
          {view.presets.map((preset) => (
            <Row
              actions={
                view.preset === preset.id ? (
                  <StatusChip tone="success">In use</StatusChip>
                ) : manage ? (
                  <Button disabled={change.isPending} onClick={() => change.mutate({ preset: preset.id })} size="sm">
                    Use
                  </Button>
                ) : null
              }
              key={preset.id}
              meta={preset.description}
              title={preset.name}
            />
          ))}
        </RowList>
      </Section>
      <Section
        description={manage ? 'A module needs the ones it builds on; turning one off turns off what needs it.' : 'Workspace owners and admins change these.'}
        title="Modules"
      >
        <RowList>
          {view.modules
            .filter((module) => module.switchable)
            .map((module) => (
              <Row
                actions={
                  <Switch
                    checked={module.switchedOn}
                    disabled={!manage || change.isPending}
                    label={<span className="visually-hidden">{module.name}</span>}
                    onChange={(checked) => change.mutate({ switches: { [module.id]: checked } })}
                  />
                }
                key={module.id}
                meta={
                  <>
                    {module.description}
                    {module.needs.length ? ` Needs ${module.needs.map((need) => names.get(need) ?? need).join(', ')}.` : null}
                    {module.offBecause && module.switchedOn ? ` ${module.offBecause}` : null}
                  </>
                }
                title={module.name}
              />
            ))}
        </RowList>
      </Section>
    </>
  )
}
