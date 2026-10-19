import type { IconName } from '../ui'

/**
 * The one list of slash commands. The composer offers them as you type "/"
 * and runs them on Enter; the command menu lists the ones that make sense
 * outside a conversation. Text starting with "/" that names no command here
 * is sent as an ordinary message.
 */
export type SlashCommandName = 'remind' | 'later' | 'schedule' | 'mute' | 'unmute' | 'notify' | 'task' | 'tag' | 'launch'

export type SlashCommand = {
  name: SlashCommandName
  /** What follows the name, as shown in the list. */
  usage: string
  description: string
  icon: IconName
  /** Where the command menu sends you for it, if it has a place outside a conversation. */
  paletteHref?: string
  /** Needs a project with tasks (not a DM or group). */
  projectOnly?: boolean
}

export const SLASH_COMMANDS: SlashCommand[] = [
  { name: 'remind', usage: 'me to … at 3pm', description: 'Set a reminder for yourself', icon: 'clock', paletteHref: '/later?new=reminder' },
  { name: 'later', usage: '[note]', description: 'Save the last message, or a note, for later', icon: 'bookmark', paletteHref: '/later' },
  { name: 'schedule', usage: 'tomorrow 9am message', description: 'Send a message here later', icon: 'send', paletteHref: '/later?view=scheduled' },
  { name: 'mute', usage: '', description: 'Stop notifications from this conversation', icon: 'circle-minus' },
  { name: 'unmute', usage: '', description: 'Hear from this conversation again', icon: 'messages' },
  { name: 'notify', usage: 'all | mentions | off', description: 'Choose what you hear from this conversation', icon: 'at-sign' },
  { name: 'task', usage: 'title', description: 'Create a task in this project', icon: 'work', projectOnly: true },
  { name: 'tag', usage: '', description: 'Ask an agent: mention it with @', icon: 'bot' },
  { name: 'launch', usage: '', description: 'Launch agents on ready tasks', icon: 'play', projectOnly: true },
]

/** The commands a conversation offers. */
export function commandsFor(options: { inProject: boolean }): SlashCommand[] {
  return SLASH_COMMANDS.filter((command) => options.inProject || !command.projectOnly)
}

/** The command `text` runs, if it names one: "/remind me …" → remind with "me …". */
export function matchSlashCommand(
  text: string,
  commands: SlashCommand[] = SLASH_COMMANDS,
): { command: SlashCommand; args: string } | null {
  const match = /^\/([a-z]+)(?:\s+([\s\S]*))?$/i.exec(text.trim())
  if (!match) return null
  const command = commands.find((candidate) => candidate.name === match[1].toLowerCase())
  return command ? { command, args: (match[2] ?? '').trim() } : null
}

/** While the name is still being typed ("/re"), the commands it could be. */
export function suggestSlashCommands(text: string, commands: SlashCommand[] = SLASH_COMMANDS): SlashCommand[] {
  const match = /^\/([a-z]*)$/i.exec(text)
  if (!match) return []
  const typed = match[1].toLowerCase()
  return commands.filter((command) => command.name.startsWith(typed))
}

/** What running a command did, for the composer. */
export type SlashOutcome =
  /** Done: clear the box. */
  | { kind: 'done' }
  /** Put this in the box instead, for example "@" to pick an agent. */
  | { kind: 'replace'; text: string }
