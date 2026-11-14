import { HostsView } from '../hosts/HostsView'
import type { SettingsPageProps } from '../types'

/** Account › Hosts: hosts you own, such as your own computer (A1.11). */
export default function HostsPage({ workspace }: SettingsPageProps) {
  return <HostsView mode="account" workspace={workspace} />
}
