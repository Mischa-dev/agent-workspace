import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { apiOriginForDisplay, requestJson } from '../api/client'
import {
  Banner,
  Button,
  ConfirmDialog,
  EmptyState,
  IconButton,
  ListSkeleton,
  Row,
  RowList,
  Section,
  StatusChip,
  Switch,
  describeError,
  describeLoadError,
  formatFullDate,
  formatRelative,
  formatShortDate,
  showToast,
} from '../ui'

type ConnectedApp = {
  id: string
  clientName: string
  access: 'full' | 'read'
  actingAs: string
  accountWide: boolean
  showVia: boolean
  createdAt: string
  lastUsedAt: string | null
}

type ConnectedAppsResponse = { mcpUrl: string; guideUrl: string; apps: ConnectedApp[] }

const key = ['connected-apps'] as const

function CopyLine({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState<'yes' | 'failed' | null>(null)
  const copy = () => {
    if (!navigator.clipboard) {
      setCopied('failed')
      return
    }
    navigator.clipboard.writeText(value).then(
      () => {
        setCopied('yes')
        window.setTimeout(() => setCopied(null), 1500)
      },
      () => setCopied('failed'),
    )
  }
  return (
    <div className="aw-copyline">
      <span className="aw-copyline__label">{label}</span>
      <code className="aw-copyline__value">{value}</code>
      <IconButton icon={copied === 'yes' ? 'check' : 'copy'} label={copied === 'yes' ? 'Copied' : `Copy ${label}`} onClick={copy} size="sm" />
      {copied === 'failed' ? (
        <span className="aw-copyline__note" role="status">
          Couldn’t copy. Select the text and copy it yourself.
        </span>
      ) : null}
    </div>
  )
}

/** Apps connected to this account over MCP, and how to connect another. */
export function ConnectedAppsSection() {
  const client = useQueryClient()
  const [revoking, setRevoking] = useState<ConnectedApp | null>(null)
  const apps = useQuery({ queryKey: key, queryFn: () => requestJson<ConnectedAppsResponse>('/api/v1/account/connected-apps') })
  const refresh = () => client.invalidateQueries({ queryKey: key })
  const toggle = useMutation({
    mutationFn: (input: { id: string; showVia: boolean }) =>
      requestJson(`/api/v1/account/connected-apps/${encodeURIComponent(input.id)}`, { method: 'PATCH', json: { showVia: input.showVia } }),
    onSuccess: refresh,
  })
  const revoke = useMutation({
    mutationFn: (app: ConnectedApp) =>
      requestJson(`/api/v1/account/connected-apps/${encodeURIComponent(app.id)}/revoke`, { method: 'POST' }),
    onSuccess: (_, app) => {
      setRevoking(null)
      showToast({ tone: 'success', title: `${app.clientName} revoked` })
      void refresh()
    },
  })
  const mcpUrl = apps.data?.mcpUrl ?? `${apiOriginForDisplay()}/api/v1/mcp`
  const guideUrl = apps.data?.guideUrl ?? `${window.location.origin}/connect.md`
  return (
    <>
      <Section
        description="Coding agents and chat apps that act as you: they see what you see and, with full access, can send messages and change tasks and pages for you."
        title="Connected apps"
      >
        {toggle.isError ? (
          <Banner tone="danger">That label setting wasn’t saved. {describeError(toggle.error).message}</Banner>
        ) : null}
        {apps.isPending ? (
          <ListSkeleton label="Loading connected apps" rows={2} />
        ) : apps.isError ? (
          <Banner onRetry={() => void apps.refetch()} title="Connected apps didn’t load." tone="danger">
            {describeLoadError(apps.error)}
          </Banner>
        ) : apps.data.apps.length === 0 ? (
          <EmptyState icon="link" title="Nothing connected yet">
            Connect an app below. It asks you to approve it here.
          </EmptyState>
        ) : (
          <RowList label="Connected apps">
            {apps.data.apps.map((app) => (
              <Row
                actions={
                  <Button onClick={() => setRevoking(app)} size="sm" variant="danger">
                    Revoke
                  </Button>
                }
                key={app.id}
                meta={
                  <>
                    <StatusChip tone={app.access === 'full' ? 'agent' : 'muted'}>{app.access === 'full' ? 'Full access' : 'Read only'}</StatusChip>
                    <span
                      title={[`Connected ${formatFullDate(app.createdAt)}`, app.lastUsedAt ? `last used ${formatFullDate(app.lastUsedAt)}` : null]
                        .filter(Boolean)
                        .join(', ')}
                    >
                      {app.actingAs === 'you' ? 'Acts as you' : `Acts as ${app.actingAs}`}
                      {app.actingAs === 'you' && !app.accountWide ? ' in one project' : ''} · connected {formatShortDate(app.createdAt)}
                      {app.lastUsedAt ? ` · last used ${formatRelative(app.lastUsedAt)}` : ''}
                    </span>
                  </>
                }
                title={app.clientName}
              >
                {app.actingAs === 'you' ? (
                  <Switch
                    checked={app.showVia}
                    description="Messages and edits it makes show “via” the app next to your name."
                    disabled={toggle.isPending && toggle.variables?.id === app.id}
                    label={`Label what it writes “via ${app.clientName}”`}
                    onChange={(showVia) => toggle.mutate({ id: app.id, showVia })}
                  />
                ) : null}
              </Row>
            ))}
          </RowList>
        )}
        <ConfirmDialog
          busy={revoke.isPending}
          confirmLabel="Revoke"
          error={revoke.isError ? `It wasn’t revoked. ${describeError(revoke.error).message}` : null}
          onClose={() => {
            setRevoking(null)
            revoke.reset()
          }}
          onConfirm={() => revoking && revoke.mutate(revoking)}
          open={revoking !== null}
          title={`Revoke ${revoking?.clientName ?? 'this app'}?`}
        >
          It stops working right away. To use it again, connect it again and approve it here.
        </ConfirmDialog>
      </Section>
      <Section
        description="Add this server address to your MCP client’s settings. The app opens a page here asking you to approve it, where you choose full or read-only access."
        title="Connect an app"
      >
        <div className="aw-connect">
          <CopyLine label="MCP address" value={mcpUrl} />
          <p className="aw-connect__note">
            In a chat app, add a custom connector with this address. Then ask it to start with the briefing tool: it lists what
            needs you.
          </p>
          <CopyLine label="Connect an agent" value={guideUrl} />
          <p className="aw-connect__note">
            Send this setup guide to any agent. It has the steps for each kind of client, whether the agent works as you or has
            its own agent account.
          </p>
        </div>
      </Section>
    </>
  )
}
