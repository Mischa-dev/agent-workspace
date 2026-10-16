import { ConnectedAppsSection } from '../../auth/ConnectedApps'
import { PageHeader } from '../../ui'

/** Account › Connected apps: apps that act as you through the MCP server. */
export default function ConnectedAppsPage() {
  return (
    <>
      <PageHeader icon="link" title="Connected apps">
        Apps you approved to work as you. Revoke one and it stops at once.
      </PageHeader>
      <ConnectedAppsSection />
    </>
  )
}
