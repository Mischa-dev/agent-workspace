export function workItemHref(projectId: string, workItemId: string) {
  return `/projects/${encodeURIComponent(projectId)}/work-items/${encodeURIComponent(workItemId)}`
}

export function workItemsHref(projectId: string) {
  return `/projects/${encodeURIComponent(projectId)}/work-items`
}

/**
 * Where C and "Create task" go. On the project's tasks page or a task, the
 * page keeps its view (filters, board, open task) and gains ?create=task;
 * anywhere else it opens the project's task list with the dialog.
 */
export function createTaskHref(projectId: string, location: { pathname: string; search: string }) {
  const base = workItemsHref(projectId)
  if (location.pathname === base || location.pathname.startsWith(`${base}/`)) {
    const parameters = new URLSearchParams(location.search)
    parameters.set('create', 'task')
    return `${location.pathname}?${parameters.toString()}`
  }
  return `${base}?create=task`
}
