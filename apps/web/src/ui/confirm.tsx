import type { ReactNode } from 'react'

import { Button } from './controls'
import { Banner } from './feedback'
import { Dialog } from './overlays'

/**
 * Asks before something that can't be undone: delete, revoke, remove someone.
 * Reversible things (archive, trash, mark done) happen at once with Undo in a
 * toast instead. Focus starts on Cancel, so a stray Enter never destroys.
 */
export function ConfirmDialog({
  busy = false,
  children,
  confirmLabel,
  error,
  onClose,
  onConfirm,
  open,
  title,
  tone = 'danger',
}: {
  busy?: boolean
  /** What happens, in a sentence or two: "It stops working right away." */
  children?: ReactNode
  /** The action, as a verb: "Revoke", "Delete page". */
  confirmLabel: string
  /** Why the last attempt failed, shown above the buttons. */
  error?: ReactNode
  onClose: () => void
  onConfirm: () => void
  open: boolean
  title: string
  tone?: 'danger' | 'primary'
}) {
  return (
    <Dialog
      description={children}
      footer={
        <>
          <Button data-autofocus disabled={busy} onClick={onClose}>
            Cancel
          </Button>
          <Button busy={busy} onClick={onConfirm} variant={tone}>
            {confirmLabel}
          </Button>
        </>
      }
      onClose={onClose}
      open={open}
      title={title}
      width={440}
    >
      {error ? <Banner tone="danger">{error}</Banner> : null}
    </Dialog>
  )
}
