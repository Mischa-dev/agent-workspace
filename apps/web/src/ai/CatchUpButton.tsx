import { lazy, Suspense, useRef, useState } from 'react'

import { IconButton } from '../ui'

const CatchUpDialog = lazy(() => import('./CatchUpDialog'))

/** AI6: "Catch me up" on a thread. The dialog and its code load on first use. */
export function CatchUpButton({ conversationId, name }: { conversationId: string; name: string }) {
  const [open, setOpen] = useState(false)
  const button = useRef<HTMLButtonElement>(null)
  return (
    <>
      <IconButton icon="activity" label="Catch me up" onClick={() => setOpen(true)} ref={button} side="bottom" />
      {open ? (
        <Suspense fallback={null}>
          <CatchUpDialog conversationId={conversationId} name={name} onClose={() => setOpen(false)} returnFocus={button} />
        </Suspense>
      ) : null}
    </>
  )
}
