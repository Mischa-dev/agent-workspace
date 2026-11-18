// The tabs of a channel's details panel (C2.8).

export type ChannelDetailsTab = 'about' | 'members' | 'pins' | 'tasks' | 'files'

/** The tabs a channel's details show here: Members only for a private channel, Tasks and Files only where those are on. */
export function channelDetailsTabs({ isPrivate, tasks, files }: { isPrivate: boolean; tasks: boolean; files: boolean }) {
  const tabs: { value: ChannelDetailsTab; label: string }[] = [{ value: 'about', label: 'About' }]
  if (isPrivate) tabs.push({ value: 'members', label: 'Members' })
  tabs.push({ value: 'pins', label: 'Pins' })
  if (tasks) tabs.push({ value: 'tasks', label: 'Tasks' })
  if (files) tabs.push({ value: 'files', label: 'Files' })
  return tabs
}
