import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { requestJson } from '../api/client'
import { isApiError } from '../api/errors'
import { formatFullDate } from '../conversations/timeFormat'
import { Banner, Button, EmptyState, IconButton, Row, RowList, Section, StatusChip, Switch } from '../ui'

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
  const [copied, setCopied] = useState(false)
  return (
    <div className="aw-copyline">
      <span className="aw-copyline__label">{label}</span>
      <code className="aw-copyline__value">{value}</code>
      <IconButton
        icon={copied ? 'check' : 'copy'}
        label={copied ? 'Copied' : `Copy ${label}`}
        onClick={() =>
          void navigator.clipboard
            ?.writeText(value)
            .then(() => {
              setCopied(true)
              window.setTimeout(() => setCopied(false), 1500)
            })
            .catch(() => undefined)
        }
        size="sm"
      />
    </div>
  )
}

/** Apps connected to this account over MCP, and how to connect another. */
export function ConnectedAppsSection() {
  const client = useQueryClient()
  const [error, setError] = useState<string | null>(null)
  const apps = useQuery({ queryKey: key, queryFn: () => requestJson<ConnectedAppsResponse>('/api/v1/account/connected-apps') })
  const refresh = () => client.invalidateQueries({ queryKey: key })
  const onError = (reason: unknown) => setError(isApiError(reason) ? reason.message : 'That didn’t work. Try again.')
  const toggle = useMutation({
    mutationFn: (input: { id: string; showVia: boolean }) =>
      requestJson(`/api/v1/account/connected-apps/${encodeURIComponent(input.id)}`, { method: 'PATCH', json: { showVia: input.showVia } }),
    onSuccess: refresh,
    onError,
  })
  const revoke = useMutation({
    mutationFn: (id: string) => requestJson(`/api/v1/account/connected-apps/${encodeURIComponent(id)}/revoke`, { method: 'POST' }),
    onSuccess: refresh,
    onError,
  })
  const mcpUrl = apps.data?.mcpUrl ?? `${window.location.origin}/api/v1/mcp`
  const guideUrl = apps.data?.guideUrl ?? `${window.location.origin}/connect.md`
  return (
    <>
      <Section
        description="Apps like Claude Code, Codex and ChatGPT that act as you: they see what you see and, with full access, can send messages and change tasks and pages for you."
        title="Connected apps"
      >
        {error ? <Banner tone="danger">{error}</Banner> : null}
        {apps.isError ? <Banner tone="danger">Connected apps didn’t load.</Banner> : null}
        {apps.data && apps.data.apps.length === 0 ? (
          <EmptyState icon="key" title="Nothing connected yet">
            Connect an app below. It will ask you to approve it here.
          </EmptyState>
        ) : null}
        {apps.data && apps.data.apps.length > 0 ? (
          <RowList label="Connected apps">
            {apps.data.apps.map((app) => (
              <Row
                actions={
                  <Button busy={revoke.isPending && revoke.variables === app.id} onClick={() => revoke.mutate(app.id)} size="sm" variant="danger">
                    Revoke
                  </Button>
                }
                key={app.id}
                meta={
                  <>
                    <StatusChip tone={app.access === 'full' ? 'agent' : 'muted'}>{app.access === 'full' ? 'Full access' : 'Read only'}</StatusChip>
                    <span>
                      {app.actingAs === 'you' ? 'Acts as you' : `Acts as ${app.actingAs}`}
                      {app.actingAs === 'you' && !app.accountWide ? ' in one project' : ''} · connected {formatFullDate(app.createdAt)}
                      {app.lastUsedAt ? ` · last used ${formatFullDate(app.lastUsedAt)}` : ''}
                    </span>
                  </>
                }
                title={app.clientName}
              >
                {app.actingAs === 'you' ? (
                  <Switch
                    checked={app.showVia}
                    description="Messages and edits it makes show “via” the app next to your name."
                    disabled={toggle.isPending}
                    label={`Label what it writes “via ${app.clientName}”`}
                    onChange={(showVia) => toggle.mutate({ id: app.id, showVia })}
                  />
                ) : null}
              </Row>
            ))}
          </RowList>
        ) : null}
      </Section>
      <Section
        description="Give your app this address. It opens a page here asking you to approve it, where you choose full or read-only access."
        title="Connect an app"
      >
        <div className="aw-connect">
          <CopyLine label="Setup guide for a bot" value={guideUrl} />
          <p className="aw-connect__note">
            Send this link to Codex, Claude, Muse, Hermes or any other agent. It explains how to connect, whether it works for
            you or has its own agent account.
          </p>
          <CopyLine label="MCP address" value={mcpUrl} />
          <CopyLine label="Claude Code" value={`claude mcp add --scope user --transport http agent-workspace ${mcpUrl}`} />
          <CopyLine label="Codex config.toml" value={`[mcp_servers.agent_workspace]\nurl = "${mcpUrl}"`} />
          <CopyLine label="Then in Codex" value="codex mcp login agent_workspace" />
          <p className="aw-connect__note">
            In Claude Code, run /mcp to sign in after adding it. In claude.ai or ChatGPT, add a custom connector with the MCP address.
            Then ask it to start with the briefing tool: it lists what needs you.
          </p>
        </div>
      </Section>
    </>
  )
}
