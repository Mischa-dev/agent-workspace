import { useQuery } from '@tanstack/react-query'

import { toastError } from '../app/mutations'
import { useWorkspaceId } from '../app/workspace'
import { opQuery, useOp, type OpOutput } from '../api/ops'
import { DetailList, Dialog, ErrorState, ListSkeleton, Row, RowList, Section, Switch } from '../ui'

type Scope = OpOutput<'agent_scope_get'>
type Conversation = Scope['privateConversations'][number]

const KIND_LABELS: Record<string, string> = { dm: 'Direct message', group: 'Group', channel: 'Private channel' }
const SPACE_LABELS: Record<string, string> = { project: 'Project', team: 'Team space', personal: 'Personal space', workspace: 'Workspace space' }

/** A channel's name; for a DM or group, who else is in it. */
function conversationTitle(conversation: Conversation): string {
  if (conversation.kind === 'channel' && conversation.name) return conversation.name
  const names = conversation.members.map((member) => (member.you ? 'you' : (member.displayName ?? 'someone')))
  if (names.length) return sentence(`With ${names.join(', ')}`)
  return conversation.name ?? KIND_LABELS[conversation.kind] ?? 'Conversation'
}

function sentence(text: string): string {
  return text ? text[0].toUpperCase() + text.slice(1) : text
}

/**
 * A2.15: what an agent can reach, as the viewer may see it. Private
 * conversations are only the ones the viewer is in too; for an account
 * that hears them only with consent, each has a switch.
 */
export function AgentScopeDialog({ accountId, name, onClose }: { accountId: string; name: string; onClose: () => void }) {
  const workspaceId = useWorkspaceId() ?? undefined
  const scope = useQuery(opQuery('agent_scope_get', { account: accountId }, { workspaceId }))
  return (
    <Dialog onClose={onClose} open title={`What ${name} can reach`} width={560}>
      {scope.isPending ? (
        <ListSkeleton rows={4} />
      ) : scope.isError ? (
        <ErrorState error={scope.error} onRetry={() => void scope.refetch()} title="Its scope didn’t load" />
      ) : (
        <ScopeDetails scope={scope.data} />
      )}
    </Dialog>
  )
}

function ScopeDetails({ scope }: { scope: Scope }) {
  return (
    <>
      <DetailList
        items={[
          { label: 'Role in this workspace', value: sentence(scope.workspaceRole) },
          {
            label: 'Managed by',
            value: scope.managers.length ? scope.managers.map((manager) => manager.displayName ?? 'Someone').join(', ') : 'Nobody named',
          },
          { label: 'Connections', value: scope.connections },
          { label: 'Follows', value: scope.follows ? `${scope.follows} you can see` : 'Nothing you can see' },
          { label: 'Also in other workspaces', value: scope.alsoInOtherWorkspaces ? 'Yes' : 'No' },
        ]}
      />
      <Section title="Spaces">
        {scope.spaces.length ? (
          <RowList label="Spaces">
            {scope.spaces.map((space) => (
              <Row key={space.spaceId} meta={`${SPACE_LABELS[space.kind] ?? sentence(space.kind)} · ${sentence(space.role)}`} title={space.name} />
            ))}
          </RowList>
        ) : (
          <p className="aw-quiet">Not in any space you can see.</p>
        )}
      </Section>
      <Section
        description={
          scope.readsWithConsent
            ? 'It hears these only where someone in them lets it. Only the ones you’re in are listed.'
            : 'Only the ones you’re in are listed.'
        }
        title="Private conversations"
      >
        {scope.privateConversations.length ? (
          <RowList label="Private conversations">
            {scope.privateConversations.map((conversation) => (
              <ConversationRow accountId={scope.accountId} consent={scope.readsWithConsent} conversation={conversation} key={conversation.conversationId} />
            ))}
          </RowList>
        ) : (
          <p className="aw-quiet">None that you’re in too.</p>
        )}
      </Section>
    </>
  )
}

function ConversationRow({ accountId, consent, conversation }: { accountId: string; consent: boolean; conversation: Conversation }) {
  // Consent is changed where the conversation lives: a DM or group in the
  // direct scope, a private channel in this workspace.
  const change = useOp('app_consent_set', {
    workspaceId: conversation.workspaceId,
    invalidates: ['agent_scope_get'],
  })
  return (
    <Row
      actions={
        consent ? (
          <Switch
            checked={conversation.heard}
            disabled={change.isPending}
            label={conversation.heard ? 'Hears it' : 'Doesn’t hear it'}
            onChange={(granted) =>
              change.mutate(
                { conversation: conversation.conversationId, app: accountId, granted },
                { onError: (reason) => toastError('That didn’t change', reason) },
              )
            }
          />
        ) : null
      }
      meta={KIND_LABELS[conversation.kind] ?? sentence(conversation.kind)}
      title={conversationTitle(conversation)}
    />
  )
}
