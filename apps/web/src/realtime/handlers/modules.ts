import { opAnswers, opAnswersIn, payloadId, type Handler, type Touch } from './touches'

// Built and connected modules (M1.6, M1.13). A built module made, changed,
// switched or archived elsewhere: the sidebar's module entries and Settings ›
// Modules read the list (and an open module) again. A connected module
// installed, re-read, approved or revoked: its list too, and the built list,
// whose nav entries can point through it.

// Only the workspace the module is in reads again; an event that doesn't
// name one refreshes every workspace's answers.
const answers = (op: string, workspaceId: string | null): Touch => (workspaceId ? opAnswersIn(op, workspaceId) : opAnswers(op))

const builtChanged: Handler = (event) => {
  const workspaceId = payloadId(event, 'workspaceId', 'workspace_id')
  return [answers('module_list', workspaceId), answers('module_open', workspaceId)]
}

const connectedChanged: Handler = (event) => {
  const workspaceId = payloadId(event, 'workspaceId', 'workspace_id')
  return [answers('module_list', workspaceId), answers('connected_module_list', workspaceId)]
}

export const moduleHandlers: Record<string, Handler> = {
  'module.created': builtChanged,
  'module.updated': builtChanged,
  'module.switched': builtChanged,
  'module.removed': builtChanged,
  'module.connected': connectedChanged,
}
