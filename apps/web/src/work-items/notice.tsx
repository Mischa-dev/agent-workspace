import { Banner, Button, IconButton } from '../ui'

export type Notice = {
  message: string
  tone: 'success' | 'warning' | 'danger' | 'info'
  action?: { label: string; run: () => void }
}

export function NoticeBar({ notice, onDismiss }: { notice: Notice | null; onDismiss: () => void }) {
  if (!notice) return null
  return (
    <Banner
      action={
        <span className="aw-notice__actions">
          {notice.action ? (
            <Button
              onClick={() => {
                notice.action?.run()
                onDismiss()
              }}
              size="sm"
            >
              {notice.action.label}
            </Button>
          ) : null}
          <IconButton icon="x" label="Dismiss" onClick={onDismiss} size="sm" tooltip={false} />
        </span>
      }
      className="aw-notice"
      role={notice.tone === 'danger' ? 'alert' : 'status'}
      tone={notice.tone}
    >
      {notice.message}
    </Banner>
  )
}
