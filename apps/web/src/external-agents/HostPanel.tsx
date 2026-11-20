import { useWorkspaceHref } from '../app/workspace'
import type { ModulePanelRef } from '../api/generated/operations'
import { PanelHost } from '../modules/panels/PanelHost'
import { Dialog, showToast } from '../ui'

/**
 * M1.16: a host's own module panel, opened from one of its offers or
 * actions. The host joined its connected module, so the server named the
 * panel (`{ moduleId, panel }`); this draws it in a dialog until the module
 * says it's done or closes it. A `done` that names the host's waiting
 * proposal (the server keeps `proposalId` only then) says where to approve
 * the new agent.
 */
export function HostPanel({
  onClose,
  panel,
  title,
  workspaceId,
}: {
  onClose: () => void
  panel: ModulePanelRef
  /** What was chosen: the offer's or the action's own words. */
  title: string
  workspaceId?: string
}) {
  const href = useWorkspaceHref()
  return (
    <Dialog onClose={onClose} open title={title} width={560}>
      <PanelHost
        label={title}
        onClose={onClose}
        onDone={({ message, result }) => {
          onClose()
          const proposed = typeof result?.proposalId === 'string'
          if (proposed) {
            showToast({
              title: 'Approve the new agent',
              description: message ?? 'It’s waiting for you in your inbox.',
              link: { label: 'Open inbox', href: href('/inbox') },
            })
          } else if (message) {
            showToast({ title: message })
          }
        }}
        place={{ ...panel, workspaceId }}
      />
    </Dialog>
  )
}
