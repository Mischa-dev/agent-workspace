import { HostsView } from '../hosts/HostsView'
import type { SettingsPageProps } from '../types'

/** Workspace › Hosts: the hosts the workspace owns, and your own (A1.11). */
export default function HostsPage({ workspace }: SettingsPageProps) {
  return <HostsView mode="workspace" workspace={workspace} />
}
