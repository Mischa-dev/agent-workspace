import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { toastError } from '../app/toastError'
import { onboardingKeys, updateOnboarding, type Onboarding } from '../onboarding/api'
import { changeRole, listRoles, roleKeys, workspaceAssignable, type WorkspaceRoleView } from '../settings/workspace/roles'
import { Button, ButtonLink, ErrorState, Row, RowList, SelectField, SettingsSkeleton, StatusChip, countLabel } from '../ui'
import { TemplatePicker } from './TemplatePicker'
import './templates.css'

// AWD-76: the first two steps of setup. Pick a template (Everything first),
// then say who gets what in plain words.

function useConfirm(onboarding: Onboarding) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => updateOnboarding(onboarding.organizationId, { confirm: 'roles' }),
    onSuccess: (updated) => queryClient.setQueryData(onboardingKeys.workspace(onboarding.organizationId), updated),
    onError: (reason) => toastError('Setup didn’t save', reason),
  })
}

export function TemplateStep({ onboarding }: { onboarding: Onboarding }) {
  if (!onboarding.canManageWorkspace) {
    return <p className="aw-onboarding__intro">An owner or admin picks how the workspace is set up. You’re set to go on.</p>
  }
  return <TemplatePicker confirm={false} organizationId={onboarding.organizationId} />
}

/** What holding a role means, in a sentence. */
function meaning(role: WorkspaceRoleView) {
  const parts = [role.description || `Based on ${role.baseRole}.`]
  if (role.agentLimit) parts.push('Agents acting for them can do less than they can.')
  if (role.modulesOff.length) parts.push(`They don’t get ${role.modulesOff.join(', ')}.`)
  return parts.join(' ')
}

export function RolesStep({ onboarding }: { onboarding: Onboarding }) {
  const queryClient = useQueryClient()
  const organizationId = onboarding.organizationId
  const confirm = useConfirm(onboarding)
  const roles = useQuery({
    queryKey: roleKeys.roles(organizationId),
    queryFn: () => listRoles(organizationId),
    enabled: onboarding.canManageWorkspace,
  })
  const makeDefault = useMutation({
    mutationFn: (role: WorkspaceRoleView) => changeRole(organizationId, role, { isDefault: true }),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: roleKeys.roles(organizationId) }),
    onError: (reason) => toastError('The default role didn’t change', reason),
  })
  if (!onboarding.canManageWorkspace) {
    return <p className="aw-onboarding__intro">Owners and admins decide who gets what. You’re set to go on.</p>
  }
  if (roles.isPending) return <SettingsSkeleton label="Loading roles" sections={1} />
  if (roles.isError) return <ErrorState error={roles.error} onRetry={() => void roles.refetch()} title="Roles didn’t load" />
  const shown = roles.data.roles.filter((role) => role.key === 'owner' || workspaceAssignable([role]).length)
  const current = roles.data.roles.find((role) => role.isDefault) ?? null
  // New people can't start as an owner or admin.
  const startable = workspaceAssignable(roles.data.roles).filter((role) => role.baseRole !== 'admin')
  return (
    <div className="aw-templates__roles">
      <RowList label="Roles in this workspace">
        {shown.map((role) => (
          <Row
            actions={role.isDefault ? <StatusChip tone="success">New people start here</StatusChip> : null}
            key={role.id}
            meta={
              <>
                {meaning(role)} {countLabel(role.workspaceHolders, 'person', 'people')} now.
              </>
            }
            title={role.name}
          />
        ))}
      </RowList>
      <SelectField
        disabled={makeDefault.isPending || !roles.data.canManage}
        hint="Invitations start with this role too. You can pick another for each invitation."
        label="New people start as"
        onChange={(event) => {
          const role = startable.find((candidate) => candidate.id === event.target.value)
          if (role) makeDefault.mutate(role)
        }}
        value={current?.id ?? ''}
      >
        {startable.map((role) => (
          <option key={role.id} value={role.id}>
            {role.name}
          </option>
        ))}
      </SelectField>
      <div className="aw-onboarding__actions">
        <Button busy={confirm.isPending} onClick={() => confirm.mutate()} variant="primary">
          That’s right
        </Button>
        <ButtonLink to="/settings/workspace/roles" variant="ghost">
          Change what each role can do
        </ButtonLink>
      </div>
    </div>
  )
}
