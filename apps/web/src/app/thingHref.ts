import { currentWorkspaceSlug, spaceHref, workspaceHref } from './href'

// Addresses of things by kind, apart from href.ts so the first page load
// doesn't carry them: only pages loaded later build them.

/** A thing's permanent address, which survives a rename or a move between spaces: `/w/<slug>/t/<kind>/<id>`. */
export function thingHref(kind: string, id: string, ws: string = currentWorkspaceSlug()): string {
  return `/w/${encodeURIComponent(ws)}/t/${encodeURIComponent(kind)}/${encodeURIComponent(id)}`
}

/**
 * Where a thing opens, built without asking the server: tasks, pages,
 * channels, threads, collections, sheets and runs in their space, workspace
 * boards and runs at the workspace, anything else by its permanent address.
 */
export function hrefFor(thing: { kind: string; id: string; spaceId?: string | null; parentId?: string | null }, ws: string = currentWorkspaceSlug()): string {
  const id = encodeURIComponent(thing.id)
  const space = thing.spaceId
  if (space) {
    switch (thing.kind) {
      case 'task':
        return spaceHref(space, `/tasks/${id}`, ws)
      case 'page':
        return spaceHref(space, `/pages/${id}`, ws)
      case 'decision':
        return spaceHref(space, '/decisions', ws)
      case 'channel':
        return spaceHref(space, `/chat/${id}`, ws)
      case 'thread':
        return spaceHref(space, `/threads/${id}`, ws)
      case 'collection':
        return spaceHref(space, `/collections/${id}`, ws)
      case 'record':
        return thing.parentId ? spaceHref(space, `/collections/${encodeURIComponent(thing.parentId)}?record=${id}`, ws) : thingHref(thing.kind, thing.id, ws)
      case 'sheet':
        return spaceHref(space, `/sheets/${id}`, ws)
      case 'whiteboard':
        return workspaceHref(ws, `/whiteboards/${id}`)
      case 'project':
      case 'space':
        return spaceHref(thing.id, '', ws)
      default:
        return thingHref(thing.kind, thing.id, ws)
    }
  }
  switch (thing.kind) {
    case 'whiteboard':
      return workspaceHref(ws, `/whiteboards/${id}`)
    case 'run':
      return workspaceHref(ws, `/runs/${id}`)
    case 'conversation':
      return workspaceHref(ws, `/dm/${id}`)
    case 'project':
    case 'space':
      return spaceHref(thing.id, '', ws)
    default:
      return thingHref(thing.kind, thing.id, ws)
  }
}
