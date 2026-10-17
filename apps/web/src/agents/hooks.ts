import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { PRESENCE_REFRESH_MS, agentKeys, createProfile, listPresence, listProfiles, updateProfile, type ProfileInput } from './api'

export function useProfiles(projectId: string, enabled = true) {
  return useQuery({ queryKey: agentKeys.profiles(projectId), queryFn: () => listProfiles(projectId), enabled: enabled && Boolean(projectId) })
}

export function usePresence(projectId: string) {
  return useQuery({
    queryKey: agentKeys.presence(projectId),
    queryFn: () => listPresence(projectId),
    enabled: Boolean(projectId),
    refetchInterval: PRESENCE_REFRESH_MS,
  })
}

function useRefresh(projectId: string) {
  const client = useQueryClient()
  return () => {
    void client.invalidateQueries({ queryKey: agentKeys.profiles(projectId) })
    void client.invalidateQueries({ queryKey: agentKeys.presence(projectId) })
  }
}

export function useCreateProfile(projectId: string) {
  const refresh = useRefresh(projectId)
  return useMutation({ mutationFn: (input: ProfileInput) => createProfile(projectId, input), onSuccess: refresh })
}

export function useUpdateProfile(projectId: string) {
  const refresh = useRefresh(projectId)
  return useMutation({
    mutationFn: ({ input, profileId }: { profileId: string; input: ProfileInput & { version: number } }) => updateProfile(projectId, profileId, input),
    onSuccess: refresh,
  })
}
