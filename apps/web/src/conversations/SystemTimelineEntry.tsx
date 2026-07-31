export function SystemTimelineEntry({ createdAt }: { createdAt: string }) {
  return (
    <article className="provider-event system-event">
      <span aria-hidden="true">i</span>
      <div>
        <strong>System activity</strong>
        <p>A system change was recorded in this conversation.</p>
      </div>
      <time dateTime={createdAt}>
        {new Date(createdAt).toLocaleTimeString([], {
          hour: 'numeric',
          minute: '2-digit',
        })}
      </time>
    </article>
  )
}
