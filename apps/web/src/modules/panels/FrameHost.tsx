// A connected module's framed page (M1.15), the escape hatch for what panel
// blocks can't draw, such as a field for a secret. The frame shown here is
// Workspace's own host page for one launch (`/panel-frame/{launch}`): it runs
// no script, its policy frames only the app's origin, and it opens once.
// The app's page inside it talks to this window by postMessage, checked
// both ways: a message counts only from that inner frame's window and the
// app's origin, and replies go only to that origin. `done` is never trusted
// on its own: Workspace asks the app for the result (`module_frame_finish`)
// and draws what the server answers.

import { useEffect, useRef, useState } from 'react'

import { callOp, newRequestId, type OpOutput } from '../../api/ops'
import { useTheme } from '../../theme/themeContext'
import { Banner, Button, Icon } from '../../ui'
import { describeError } from '../../ui/errorText'

type Answer = OpOutput<'module_panel_open'>['answer']
export type FrameAnswer = Extract<Answer, { kind: 'frame' }>

const MIN_HEIGHT = 100
const MAX_HEIGHT = 2000

type FrameMessage = { aw: 1; type: string; height?: unknown; ref?: unknown }

function readMessage(data: unknown): FrameMessage | null {
  if (!data || typeof data !== 'object') return null
  const message = data as Record<string, unknown>
  return message.aw === 1 && typeof message.type === 'string' ? (message as FrameMessage) : null
}

export default function FrameHost({
  frame,
  onAnswer,
  onCancel,
  workspaceId,
}: {
  frame: FrameAnswer
  /** What the app answered when Workspace asked it for the frame's result. */
  onAnswer: (answer: Answer) => void
  onCancel: () => void
  workspaceId?: string
}) {
  const outer = useRef<HTMLIFrameElement>(null)
  const [height, setHeight] = useState(frame.height)
  const [failed, setFailed] = useState<unknown>(null)
  const finishing = useRef(false)
  const { shownMode } = useTheme()
  // The host page opens once, so its address is fixed when the frame is
  // shown. It takes the page's light or dark so the browser keeps it
  // see-through.
  const [address] = useState(() => `${frame.address}?scheme=${shownMode}`)

  useEffect(() => {
    const listen = (event: MessageEvent) => {
      if (event.origin !== frame.origin) return
      // The app's page is the host page's one frame.
      const app = outer.current?.contentWindow?.frames[0]
      if (!app || event.source !== app) return
      const message = readMessage(event.data)
      if (!message) return
      switch (message.type) {
        case 'ready':
          app.postMessage(
            {
              aw: 1,
              type: 'context',
              theme: shownMode,
              locale: navigator.language,
            },
            frame.origin,
          )
          break
        case 'resize':
          if (typeof message.height === 'number' && Number.isFinite(message.height)) {
            setHeight(Math.round(Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, message.height))))
          }
          break
        case 'done':
          if (typeof message.ref !== 'string' || finishing.current) break
          finishing.current = true
          setFailed(null)
          callOp('module_frame_finish', { launchId: frame.launchId, ref: message.ref }, { workspaceId, requestId: newRequestId() })
            .then((value) => onAnswer(value.answer))
            .catch((error: unknown) => setFailed(error))
            .finally(() => {
              finishing.current = false
            })
          break
        case 'cancel':
          onCancel()
          break
      }
    }
    window.addEventListener('message', listen)
    return () => window.removeEventListener('message', listen)
  }, [frame.launchId, frame.origin, onAnswer, onCancel, shownMode, workspaceId])

  return (
    <div className="aw-frame-host" data-part="frame-host">
      {failed ? <Banner tone="danger">{describeError(failed).message}</Banner> : null}
      <iframe className="aw-frame-host__frame" ref={outer} referrerPolicy="no-referrer" src={address} style={{ height }} title={frame.title} />
      <div className="aw-frame-host__footer">
        {/* Whose page this is, since it isn't Workspace's own. */}
        <span className="aw-frame-host__origin">
          <Icon name="link" size={14} />
          From {new URL(frame.origin).host}
        </span>
        <Button onClick={onCancel} size="sm" variant="ghost">
          Close
        </Button>
      </div>
    </div>
  )
}
