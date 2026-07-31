export function NavigationSyncAlert({
  error,
  retry,
}: {
  error: string | null
  retry: () => void
}) {
  if (!error) return null
  return (
    <div className="inline-error navigation-sync-error" role="alert">
      <p>{error}</p>
      <button onClick={retry} type="button">
        Try saving again
      </button>
    </div>
  )
}
