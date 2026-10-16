import { useMutation, useQuery } from '@tanstack/react-query'

import { toastError } from '../app/mutations'
import { getGitHubConnection, startGitHubConnect } from './api'

export const githubConnectionKey = (projectId: string) => ['project', projectId, 'github-connection'] as const
export const availableRepositoriesKey = (projectId: string) => ['project', projectId, 'github-available-repositories'] as const

/** Sends the browser to GitHub: the App's install page, or a sign-in that proves access to installations that exist. */
export function useGitHubConnect(projectId: string) {
  return useMutation({
    mutationFn: (mode: 'install' | 'authorize') => startGitHubConnect(projectId, mode),
    onSuccess: ({ url }) => window.location.assign(url),
    onError: (error) => {
      toastError('Couldn’t open GitHub', error)
    },
  })
}

export function useGitHubConnection(projectId: string, enabled = true) {
  return useQuery({
    queryKey: githubConnectionKey(projectId),
    queryFn: () => getGitHubConnection(projectId),
    enabled: enabled && Boolean(projectId),
  })
}

/** What the `?github=` return from GitHub means, in words. */
export function githubReturnMessage(result: string | null, reason: string | null) {
  if (result === 'connected') return { tone: 'success' as const, text: 'GitHub is connected. Add a repository below.' }
  if (result !== 'error') return null
  switch (reason) {
    case 'expired':
      return { tone: 'danger' as const, text: 'Connecting took too long and the link expired. Try again.' }
    case 'denied':
      return { tone: 'danger' as const, text: 'GitHub access wasn’t granted, so nothing was connected.' }
    case 'not_yours':
      return {
        tone: 'danger' as const,
        text: 'That installation belongs to a GitHub account you can’t reach. Ask someone with access to it to connect it.',
      }
    default:
      return { tone: 'danger' as const, text: 'GitHub couldn’t be connected. Try again.' }
  }
}

