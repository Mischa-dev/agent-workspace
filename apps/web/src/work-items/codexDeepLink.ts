import type { WorkItem } from './types'

const canonicalGitHubOrigin = /^https:\/\/github\.com\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+\.git$/
const maxCodexPromptLength = 2_000

export function buildOpenInCodexLink(item: WorkItem, applicationOrigin: string) {
  const repositories = item.linkedRepositories
  if (item.archivedAt) return { href: null, reason: 'Restore this task before opening it locally.' }
  if (repositories.length === 0) return { href: null, reason: 'Link one trusted repository before opening this task locally.' }
  if (repositories.length > 1) return { href: null, reason: 'Keep exactly one linked repository, so the right one opens.' }

  const repository = repositories[0]
  if (!repository.trusted) return { href: null, reason: 'The linked repository must be trusted before it can open locally.' }
  if (repository.archivedAt) return { href: null, reason: 'Restore the linked repository before opening this task locally.' }
  if (!repository.available) return { href: null, reason: 'Prepare and enable the linked repository before opening it locally.' }
  if (!canonicalGitHubOrigin.test(repository.cloneUrl)) {
    return { href: null, reason: 'The linked repository doesn’t have a supported canonical Git origin URL.' }
  }

  const origin = applicationOrigin.replace(/\/$/, '')
  const projectUrl = `${origin}/projects/${encodeURIComponent(item.projectId)}`
  const workItemUrl = `${projectUrl}/work-items/${encodeURIComponent(item.id)}`
  const prompt = [
    'Use the Agent Workspace MCP server.',
    `Project ID: ${item.projectId}`,
    `Project URL: ${projectUrl}`,
    `Task ID: ${item.id}`,
    `Task URL: ${workItemUrl}`,
    `Task key: ${item.key}`,
    'Load the task and its project instructions through MCP before changing anything.',
    'Work in the repository matched by this Git origin. Set the task to In Progress only when meaningful work begins.',
    'Post only useful progress or blocker updates. Respect the current version; refetch and make an informed retry after a version conflict.',
    'When complete, link the canonical pull request through MCP and mark the task ready for review.',
  ].join('\n').slice(0, maxCodexPromptLength)
  return {
    href: `codex://new?originUrl=${encodeURIComponent(repository.cloneUrl)}&prompt=${encodeURIComponent(prompt)}`,
    reason: null,
  }
}
