import { useMemo, useState, type FormEvent } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { describeError } from '../../app/mutations'
import { useWorkspaceLayout } from '../../experience/api'
import { REGISTRY } from '../../experience/registry'
import {
  Banner,
  Button,
  Checkbox,
  ConfirmDialog,
  Dialog,
  ErrorState,
  Icon,
  ListSkeleton,
  PageHeader,
  Row,
  RowList,
  Section,
  SegmentedControl,
  SelectField,
  StatusChip,
  Switch,
  TextArea,
  TextField,
  countLabel,
  showToast,
} from '../../ui'
import type { SettingsPageProps } from '../types'
import {
  changeRole,
  createRole,
  listRoles,
  removeRole,
  roleKeys,
  type BaseRole,
  type PermissionInfo,
  type PermissionKey,
  type RoleDefaults,
  type RoleInput,
  type RolesResponse,
  type StartingPoint,
  type WorkspaceRoleView,
} from './roles'
import './roles.css'

const BASE_LABELS: Record<BaseRole, string> = { owner: 'Owner', admin: 'Admin', member: 'Member', viewer: 'Viewer' }
const PINNABLE_VIEWS = [
  { key: 'my_tasks', label: 'My tasks' },
  { key: 'ready_for_agents', label: 'Ready for agents' },
  { key: 'blocked', label: 'Blocked' },
  { key: 'recently_changed', label: 'Recently changed' },
]
const HIDEABLE_SECTIONS = REGISTRY.parts.filter((part) => part.id.startsWith('home.') || part.id.startsWith('workspace.'))

type Editing =
  | { kind: 'new'; from: StartingPoint | null; copyOf: WorkspaceRoleView | null }
  | { kind: 'edit'; role: WorkspaceRoleView }

/** Workspace › Roles: what each role lets people and agents do, the defaults its holders start with, and custom roles. */
export default function RolesPage({ workspace }: SettingsPageProps) {
  const organizationId = workspace?.id ?? ''
  const queryClient = useQueryClient()
  const [editing, setEditing] = useState<Editing | null>(null)
  const [removing, setRemoving] = useState<WorkspaceRoleView | null>(null)
  const roles = useQuery({
    queryKey: roleKeys.roles(organizationId),
    queryFn: () => listRoles(organizationId),
    enabled: Boolean(organizationId),
  })
  const refresh = () => queryClient.invalidateQueries({ queryKey: ['organization', organizationId] })
  const remove = useMutation({
    mutationFn: (role: WorkspaceRoleView) => removeRole(organizationId, role),
    onSuccess: (_, role) => {
      setRemoving(null)
      showToast({ tone: 'success', title: `${role.name} removed` })
    },
    onSettled: refresh,
  })

  if (!workspace) return null
  const data = roles.data
  const owner = workspace.role === 'owner'

  return (
    <>
      <PageHeader icon="shield-check" title="Roles">
        What people and agents can do in {workspace.name}, and where they start. Built-in roles keep their permissions;
        make your own from one of them to give someone less.
      </PageHeader>
      {roles.isPending ? <ListSkeleton label="Loading roles" rows={4} /> : null}
      {roles.isError ? <ErrorState error={roles.error} onRetry={() => void roles.refetch()} title="Roles didn’t load" /> : null}
      {data ? (
        <>
          <Section
            actions={
              data.canManage ? (
                <Button icon="plus" onClick={() => setEditing({ kind: 'new', from: null, copyOf: null })} size="sm">
                  New role
                </Button>
              ) : null
            }
            description="Someone’s role in a project is the one that counts there; their workspace role covers the workspace itself and is where new projects start them."
            id="roles-heading"
            title="Roles"
          >
            <RowList label="Roles">
              {data.roles.map((role) => (
                <Row
                  actions={
                    <span className="aw-roles__actions">
                      <Button onClick={() => setEditing({ kind: 'edit', role })} size="sm" variant="ghost">
                        {role.capabilities.edit ? 'Edit' : 'View'}
                      </Button>
                      {data.canManage && role.key !== 'owner' ? (
                        <Button
                          icon="copy"
                          onClick={() => setEditing({ kind: 'new', from: null, copyOf: role })}
                          size="sm"
                          variant="ghost"
                        >
                          Duplicate
                        </Button>
                      ) : null}
                      {role.capabilities.remove ? (
                        <Button icon="trash" onClick={() => setRemoving(role)} size="sm" variant="ghost">
                          Remove
                        </Button>
                      ) : null}
                    </span>
                  }
                  key={role.id}
                  leading={
                    <span className="aw-row__tile">
                      <Icon name={role.builtIn ? 'shield-check' : 'users'} size={16} />
                    </span>
                  }
                  meta={
                    <span className="aw-row__metaline aw-wrap">
                      {role.description ? <span>{role.description}</span> : null}
                      <span>{holderLine(role)}</span>
                      {!role.builtIn ? <span>Based on {BASE_LABELS[role.baseRole]}</span> : null}
                      {role.agentLimit ? <span>Apps acting for holders are limited</span> : null}
                    </span>
                  }
                  title={
                    <span className="aw-row__titleline">
                      {role.name}
                      {role.builtIn ? <StatusChip tone="muted">Built-in</StatusChip> : null}
                      {role.isDefault ? <StatusChip tone="success">Default</StatusChip> : null}
                    </span>
                  }
                />
              ))}
            </RowList>
          </Section>
          {data.canManage ? (
            <Section description="Ready-made roles to adjust and save." id="roles-starting-heading" title="Start from">
              <ul className="aw-roles__starts">
                {data.startingPoints.map((point) => (
                  <li className="aw-card aw-roles__start" key={point.key}>
                    <span className="aw-roles__starttitle">{point.name}</span>
                    <span className="aw-quiet">{point.description}</span>
                    <Button
                      disabled={point.baseRole === 'admin' && !owner}
                      onClick={() => setEditing({ kind: 'new', from: point, copyOf: null })}
                      size="sm"
                    >
                      Use {point.name}
                    </Button>
                  </li>
                ))}
              </ul>
            </Section>
          ) : null}
          {editing ? (
            <RoleEditor
              data={data}
              editing={editing}
              onClose={() => setEditing(null)}
              onSaved={(role, created) => {
                setEditing(null)
                showToast({ tone: 'success', title: created ? `${role.name} created` : `${role.name} saved` })
                void refresh()
              }}
              organizationId={organizationId}
              owner={owner}
            />
          ) : null}
          <ConfirmDialog
            busy={remove.isPending}
            confirmLabel="Remove role"
            error={remove.isError ? `The role wasn’t removed. ${describeError(remove.error).message}` : null}
            onClose={() => {
              setRemoving(null)
              remove.reset()
            }}
            onConfirm={() => removing && remove.mutate(removing)}
            open={removing !== null}
            title={removing ? `Remove ${removing.name}?` : 'Remove role?'}
          >
            Nobody holds it, so nobody’s access changes. Layout rules for it in projects go with it.
          </ConfirmDialog>
        </>
      ) : null}
    </>
  )
}

function holderLine(role: WorkspaceRoleView) {
  const parts = []
  if (role.workspaceHolders) parts.push(`${countLabel(role.workspaceHolders, 'holder')} in the workspace`)
  if (role.projectHolders) parts.push(`${countLabel(role.projectHolders, 'membership')} in projects`)
  return parts.length ? parts.join(' · ') : 'Nobody holds it yet'
}

function initialInput(editing: Editing, data: RolesResponse): RoleInput {
  if (editing.kind === 'edit') {
    const role = editing.role
    return {
      name: role.name,
      description: role.description,
      baseRole: role.baseRole === 'owner' ? 'admin' : role.baseRole,
      permissions: role.permissions,
      agentLimit: role.agentLimit,
      defaults: role.defaults,
      isDefault: role.isDefault,
    }
  }
  if (editing.from) {
    const point = editing.from
    return {
      name: point.name,
      description: point.description,
      baseRole: point.baseRole,
      permissions: point.permissions,
      agentLimit: point.agentLimit,
      defaults: point.defaults,
      isDefault: false,
    }
  }
  const copy = editing.copyOf
  const baseRole = copy && copy.baseRole !== 'owner' ? copy.baseRole : 'member'
  return {
    name: copy ? `${copy.name} (copy)` : '',
    description: copy?.description ?? '',
    baseRole,
    permissions: copy?.permissions ?? builtInPermissions(data.permissions, baseRole),
    agentLimit: copy?.agentLimit ?? null,
    defaults: copy?.defaults ?? {},
    isDefault: false,
  }
}

function builtInPermissions(catalogue: PermissionInfo[], base: BaseRole): PermissionKey[] {
  return catalogue.filter((permission) => permission.builtIn.includes(base)).map((permission) => permission.key)
}

function RoleEditor({
  data,
  editing,
  onClose,
  onSaved,
  organizationId,
  owner,
}: {
  data: RolesResponse
  editing: Editing
  onClose: () => void
  onSaved: (role: WorkspaceRoleView, created: boolean) => void
  organizationId: string
  owner: boolean
}) {
  const [input, setInput] = useState<RoleInput>(() => initialInput(editing, data))
  const [channels, setChannels] = useState(() => (input.defaults.channels ?? []).join(', '))
  const [error, setError] = useState<string | null>(null)
  const layout = useWorkspaceLayout(organizationId)
  const role = editing.kind === 'edit' ? editing.role : null
  const readOnly = role ? !role.capabilities.edit : !data.canManage
  const permissionsLocked = readOnly || Boolean(role?.builtIn)
  const isOwnerRole = role?.key === 'owner'
  const base = builtInPermissions(data.permissions, input.baseRole)
  const yours = new Set(data.yourPermissions)
  const held = new Set(input.permissions)

  const save = useMutation({
    mutationFn: async () => {
      const defaults: RoleDefaults = {
        ...input.defaults,
        channels: channels
          .split(',')
          .map((channel) => channel.trim().replace(/^#/, ''))
          .filter(Boolean),
      }
      if (role) {
        return changeRole(organizationId, role, {
          name: input.name,
          description: input.description,
          ...(role.builtIn ? {} : { permissions: input.permissions }),
          ...(isOwnerRole ? {} : { agentLimit: input.agentLimit }),
          defaults,
          ...(isOwnerRole ? {} : { isDefault: input.isDefault }),
        })
      }
      return createRole(organizationId, { ...input, defaults })
    },
    onSuccess: (saved) => onSaved(saved, !role),
    onError: (reason) => setError(`${role ? 'The role wasn’t saved.' : 'The role wasn’t created.'} ${describeError(reason).message}`),
  })

  const setBase = (next: Exclude<BaseRole, 'owner'>) =>
    setInput((current) => {
      const allowed = new Set(builtInPermissions(data.permissions, next))
      return { ...current, baseRole: next, permissions: current.permissions.filter((key) => allowed.has(key)) }
    })
  const toggle = (key: PermissionKey, on: boolean) =>
    setInput((current) => ({
      ...current,
      permissions: on ? [...new Set([...current.permissions, key])] : current.permissions.filter((entry) => entry !== key),
    }))
  const toggleLimit = (key: PermissionKey, on: boolean) =>
    setInput((current) => {
      const limit = current.agentLimit ?? current.permissions
      return { ...current, agentLimit: on ? [...new Set([...limit, key])] : limit.filter((entry) => entry !== key) }
    })
  const setDefaults = (change: Partial<RoleDefaults>) => setInput((current) => ({ ...current, defaults: { ...current.defaults, ...change } }))

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (readOnly) return onClose()
    setError(null)
    save.mutate()
  }
  const title = role ? (readOnly ? role.name : `Edit ${role.name}`) : 'New role'
  const savedPresets = (layout.data?.savedPresets ?? []).filter((preset) => !preset.removed)

  return (
    <Dialog
      dirty={!readOnly}
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            {readOnly ? 'Close' : 'Cancel'}
          </Button>
          {!readOnly ? (
            <Button busy={save.isPending} disabled={!input.name.trim()} type="submit" variant="primary">
              {role ? 'Save role' : 'Create role'}
            </Button>
          ) : null}
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title={title}
      width={760}
    >
      <div className="aw-form aw-roles__editor">
        {readOnly ? (
          <Banner tone="info">Only owners and admins change roles{role && role.baseRole === 'admin' ? ', and only owners change this one' : ''}.</Banner>
        ) : null}
        <div className="aw-form__grid">
          <TextField
            disabled={readOnly}
            id="role-name"
            label="Name"
            maxLength={60}
            onChange={(event) => setInput({ ...input, name: event.target.value })}
            value={input.name}
          />
          {role ? (
            <TextField disabled id="role-base" label="Based on" value={role.builtIn ? 'Built-in role' : BASE_LABELS[role.baseRole]} />
          ) : (
            <SegmentedControl<Exclude<BaseRole, 'owner'>>
              label="Based on"
              onChange={setBase}
              options={(['admin', 'member', 'viewer'] as const)
                .filter((key) => key !== 'admin' || owner)
                .map((key) => ({ value: key, label: BASE_LABELS[key] }))}
              value={input.baseRole}
            />
          )}
        </div>
        <TextArea
          disabled={readOnly}
          id="role-description"
          label="Description"
          maxLength={300}
          onChange={(event) => setInput({ ...input, description: event.target.value })}
          rows={2}
          value={input.description}
        />
        {/* The built-in Viewer is for projects; a workspace default must be a role based on it. */}
        {!isOwnerRole && input.baseRole !== 'admin' && !(role?.builtIn && input.baseRole === 'viewer') ? (
          <Checkbox
            checked={input.isDefault}
            description="Invitations and new members start with it."
            disabled={readOnly || Boolean(role?.isDefault)}
            label="Default role"
            onChange={(isDefault) => setInput({ ...input, isDefault })}
          />
        ) : null}

        <fieldset className="aw-roles__group">
          <legend className="aw-roles__legend">What holders can do</legend>
          {role?.builtIn ? (
            <p className="aw-quiet">Built-in roles keep their permissions. Duplicate this one to change them.</p>
          ) : (
            <p className="aw-quiet">A role keeps only what its base role has. You can’t give a role something you can’t do yourself.</p>
          )}
          {data.modules.map((module) => {
            const permissions = data.permissions.filter((permission) => permission.module === module.key)
            if (!permissions.length) return null
            return (
              <div className="aw-roles__module" key={module.key}>
                <h3 className="aw-roles__moduletitle">{module.name}</h3>
                {permissions.map((permission) => (
                  <Checkbox
                    checked={held.has(permission.key)}
                    description={
                      <>
                        {permission.meaning}
                        {permission.peopleOnly ? ' People only: never an agent.' : ''}
                        {permission.scope === 'workspace' ? ' Workspace-wide.' : ''}
                      </>
                    }
                    disabled={
                      permissionsLocked || permission.required || !base.includes(permission.key) || !yours.has(permission.key)
                    }
                    key={permission.key}
                    label={permission.label}
                    onChange={(on) => toggle(permission.key, on)}
                  />
                ))}
              </div>
            )
          })}
        </fieldset>

        {!isOwnerRole ? (
          <fieldset className="aw-roles__group">
            <legend className="aw-roles__legend">Apps and agents acting for holders</legend>
            <Switch
              checked={input.agentLimit === null}
              description="Apps a holder connects, and agents working for them, can do what the holder can. Turn it off to allow less."
              disabled={readOnly}
              label="Same as the holder"
              onChange={(same) => setInput({ ...input, agentLimit: same ? null : input.permissions })}
            />
            {input.agentLimit !== null ? (
              <div className="aw-roles__limit">
                {data.permissions
                  .filter((permission) => held.has(permission.key) && !permission.required)
                  .map((permission) => (
                    <Checkbox
                      checked={input.agentLimit?.includes(permission.key) ?? false}
                      disabled={readOnly}
                      key={permission.key}
                      label={permission.label}
                      onChange={(on) => toggleLimit(permission.key, on)}
                    />
                  ))}
              </div>
            ) : null}
          </fieldset>
        ) : null}

        <fieldset className="aw-roles__group">
          <legend className="aw-roles__legend">Where holders start</legend>
          <SelectField
            disabled={readOnly}
            hint="The layout holders see in projects that don’t set one for them."
            id="role-layout"
            label="Layout"
            onChange={(event) => setDefaults({ layoutPreset: event.target.value || undefined })}
            value={input.defaults.layoutPreset ?? ''}
          >
            <option value="">The project’s layout</option>
            {REGISTRY.presets.map((preset) => (
              <option key={preset.id} value={preset.id}>
                {preset.name}
              </option>
            ))}
            {savedPresets.map((preset) => (
              <option key={preset.id} value={preset.id}>
                {preset.name}
              </option>
            ))}
          </SelectField>
          <TextField
            disabled={readOnly}
            hint="Private channels they join when they arrive in a project, if the person adding them is in it. Separate names with commas."
            id="role-channels"
            label="Channels"
            onChange={(event) => setChannels(event.target.value)}
            placeholder="design, leads"
            value={channels}
          />
          <div className="aw-roles__module">
            <h3 className="aw-roles__moduletitle">Pinned views</h3>
            {PINNABLE_VIEWS.map((view) => {
              const pinned = input.defaults.pinnedViews ?? ['my_tasks', 'ready_for_agents']
              return (
                <Checkbox
                  checked={pinned.includes(view.key)}
                  disabled={readOnly}
                  key={view.key}
                  label={view.label}
                  onChange={(on) =>
                    setDefaults({ pinnedViews: on ? [...pinned, view.key] : pinned.filter((entry) => entry !== view.key) })
                  }
                />
              )
            })}
          </div>
          <div className="aw-roles__module">
            <h3 className="aw-roles__moduletitle">Home sections they don’t see</h3>
            {HIDEABLE_SECTIONS.map((part) => {
              const hidden = input.defaults.homeHidden ?? []
              return (
                <Checkbox
                  checked={hidden.includes(part.id)}
                  description={part.description}
                  disabled={readOnly}
                  key={part.id}
                  label={part.label}
                  onChange={(on) =>
                    setDefaults({ homeHidden: on ? [...hidden, part.id] : hidden.filter((entry) => entry !== part.id) })
                  }
                />
              )
            })}
          </div>
        </fieldset>

        <Preview data={data} input={input} />
        {error ? <Banner tone="danger">{error}</Banner> : null}
      </div>
    </Dialog>
  )
}

/** What a holder of this role sees and can do, module by module, and what their apps can. */
function Preview({ data, input }: { data: RolesResponse; input: RoleInput }) {
  const lines = useMemo(() => {
    const held = new Set(input.permissions)
    const apps = new Set(input.agentLimit ?? input.permissions)
    return data.modules.map((module) => {
      const permissions = data.permissions.filter((permission) => permission.module === module.key)
      const can = permissions.filter((permission) => held.has(permission.key))
      const appsCan = can.filter((permission) => permission.required || apps.has(permission.key))
      return { module, can, appsCan }
    })
  }, [data, input])
  return (
    <section aria-labelledby="role-preview-heading" className="aw-card aw-roles__preview">
      <h3 className="aw-roles__moduletitle" id="role-preview-heading">
        What a holder gets
      </h3>
      <ul className="aw-roles__previewlist">
        {lines.map(({ module, can, appsCan }) => (
          <li key={module.key}>
            <span className="aw-roles__previewmodule">{module.name}</span>
            <span className={can.length ? undefined : 'aw-quiet'}>
              {can.length ? can.map((permission) => permission.label).join(', ') : 'Nothing'}
              {input.agentLimit !== null && appsCan.length < can.length ? (
                <span className="aw-quiet"> · their apps: {appsCan.length ? appsCan.map((permission) => permission.label).join(', ') : 'nothing'}</span>
              ) : null}
            </span>
          </li>
        ))}
      </ul>
    </section>
  )
}

