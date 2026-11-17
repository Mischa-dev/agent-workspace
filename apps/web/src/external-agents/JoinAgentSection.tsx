import { useQuery, useQueryClient } from '@tanstack/react-query'

import { toastError } from '../app/mutations'
import { useWorkspaceId } from '../app/workspace'
import { opQuery, useOp } from '../api/ops'
import { queryKeys } from '../api/queryKeys'
import { Avatar, Button, Section, showToast } from '../ui'

/**
 * A2.18: "Add one you already have": agents and apps you manage in other
 * workspaces, added here with the workspace's role for new agents. Shown
 * only when there is one to add; a workspace that asks an admin first says
 * so, and the agent arrives once they approve.
 */
export function JoinAgentSection({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient()
  const workspaceId = useWorkspaceId() ?? undefined
  const joinable = useQuery(opQuery('account_joinable_list', {}, { workspaceId }))
  const add = useOp('account_workspace_add', {
    workspaceId,
    invalidates: ['account_joinable_list'],
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.externalAgents(projectId) })
      showToast(
        result.status === 'asked'
          ? { title: 'Asked an admin', description: 'It joins once an admin here approves.' }
          : { title: 'Added', description: 'It keeps its identity and connections, and starts in this workspace’s own space.' },
      )
    },
  })
  const accounts = joinable.data?.accounts ?? []
  if (accounts.length === 0) return null
  return (
    <Section
      description="Agents you manage in other workspaces. Adding one keeps its identity, token and connections; it gets this workspace’s role for new agents."
      title="Add one you already have"
    >
      <ul className="aw-agents__joinable">
        {accounts.map((account) => (
          <li className="aw-agents__joinable-row" key={account.accountId}>
            <Avatar kind="agent" name={account.displayName} seed={account.accountId} size={28} />
            <span className="aw-agents__joinable-name">
              <strong>{account.displayName}</strong> <span className="aw-agents__joinable-handle">@{account.handle}</span>
            </span>
            <Button
              busy={add.isPending && add.variables?.account === account.accountId}
              disabled={add.isPending || account.handleTaken}
              onClick={() =>
                add.mutate(
                  { account: account.accountId },
                  { onError: (error) => toastError(`${account.displayName} wasn’t added`, error) },
                )
              }
              title={account.handleTaken ? `Someone here is already @${account.handle}.` : undefined}
            >
              Add
            </Button>
          </li>
        ))}
      </ul>
    </Section>
  )
}
