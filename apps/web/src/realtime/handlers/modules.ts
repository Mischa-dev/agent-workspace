import { opAnswers, type Handler } from './touches'

// Built and connected modules (M1.6, M1.13). A built module made, changed,
// switched or archived elsewhere: the sidebar's module entries and Settings ›
// Modules read the list (and an open module) again. A connected module
// installed, re-read, approved or revoked: its list too, and the built list,
// whose nav entries can point through it.

const builtChanged: Handler = () => [opAnswers('module_list'), opAnswers('module_open')]

const connectedChanged: Handler = () => [opAnswers('module_list'), opAnswers('connected_module_list')]

export const moduleHandlers: Record<string, Handler> = {
  'module.created': builtChanged,
  'module.updated': builtChanged,
  'module.switched': builtChanged,
  'module.removed': builtChanged,
  'module.connected': connectedChanged,
}
