export function workItemHref(projectId: string, workItemId: string) {
  return `/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(workItemId)}`
}

export function workItemsHref(projectId: string) {
  return `/projects/${encodeURIComponent(projectId)}/work-items`
}
