// The ids in registry.json, for type checking. Import the types anywhere;
// import the arrays only from lazy pages and tests, so the initial bundle
// stays small. registry.test.ts checks they match registry.json.

/** Every part a layout can show or hide, in registry order. */
export const PART_IDS = [
  'area.chat',
  'area.tasks',
  'area.board',
  'area.timeline',
  'area.milestones',
  'area.planning',
  'area.pages',
  'area.decisions',
  'area.whiteboards',
  'area.collections',
  'area.sheets',
  'area.runs',
  'area.agents',
  'area.automations',
  'area.code',
  'area.bug_reports',
  'area.files',
  'task.type',
  'task.priority',
  'task.estimate',
  'task.custom_fields',
  'task.labels',
  'task.milestone',
  'task.dates',
  'task.dependencies',
  'task.criteria',
  'task.pull_requests',
  'task.runs',
  'task.suggestions',
  'talk.slash_commands',
  'talk.scheduling',
  'talk.agent_mentions',
  'talk.threads',
  'talk.replies',
  'page.live_editing',
  'page.comments',
  'page.import_export',
  'page.references',
  'tech.task_keys',
  'tech.run_usage',
  'tech.ids',
  'home.working_now',
  'home.capacity',
  'workspace.my_work',
  'workspace.later',
  'workspace.inbox_triage',
  'workspace.runs',
] as const

export type PartId = (typeof PART_IDS)[number]

/** Nouns a layout can rename. */
export const NOUN_IDS = ['task', 'milestone', 'page', 'channel'] as const

export type NounId = (typeof NOUN_IDS)[number]
