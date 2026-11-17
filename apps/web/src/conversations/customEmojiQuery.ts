// The workspace's own emoji as reaction picker choices (C2.19).
import { useQuery } from '@tanstack/react-query'

import { callOp } from '../api/ops'
import { useHomeWorkspaceId } from '../experience/layout'
import type { ReactionChoice } from './reactions'

export const customEmojiKeys = { list: (workspaceId: string | null) => ['custom-emoji', workspaceId] as const }

/** The live custom emoji of `workspace` (the message's; your home one when unknown), as picker choices; none until `enabled`. */
export function useCustomEmoji(enabled: boolean, workspace?: string | null): ReactionChoice[] {
  const home = useHomeWorkspaceId()
  const workspaceId = workspace || home
  const query = useQuery({
    queryKey: customEmojiKeys.list(workspaceId),
    queryFn: () => callOp('custom_emoji_list', {}, { workspaceId: workspaceId ?? undefined }),
    enabled: enabled && Boolean(workspaceId),
    staleTime: 5 * 60_000,
  })
  return (query.data?.emoji ?? []).map((emoji) => ({
    emoji: `:${emoji.name}:`,
    name: emoji.name,
    label: `:${emoji.name}:`,
    imageUrl: emoji.imageUrl,
  }))
}
