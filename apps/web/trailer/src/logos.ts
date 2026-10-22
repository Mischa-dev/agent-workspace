import chatgpt from '../assets/logos/chatgpt.svg?url'
import claude from '../assets/logos/claude.svg?url'
import codex from '../assets/logos/codex.svg?url'
import grok from '../assets/logos/grok.svg?url'
import hermes from '../assets/logos/hermes.svg?url'
import muse from '../assets/logos/muse.svg?url'
import openclaw from '../assets/logos/openclaw.svg?url'
import { agents, builtinAgent } from './data'

/*
 * Each demo agent's own mark, so it reads at a glance wherever its avatar is
 * drawn: its maker's logo on a tile (assets/logos, from the lobehub icon set),
 * or a drawn mark for the two that have no maker's logo, Scout (your own
 * harness) and the workspace's built-in agent.
 */

export type AgentMark = { src: string; light?: boolean } | { glyph: 'scout' | 'workspace' }

export const LOGO_URLS = [chatgpt, claude, codex, grok, hermes, muse, openclaw]

let marks: Map<string, AgentMark> | null = null

/** The mark for an avatar's seed (an agent's id), if it's one of the demo agents. Built on first use, after the demo data exists. */
export function markFor(seed: string | undefined): AgentMark | undefined {
  if (!seed) return undefined
  marks ??= new Map<string, AgentMark>([
    [agents.claude.id, { src: claude, light: true }],
    [agents.codex.id, { src: codex, light: true }],
    [agents.chatgpt.id, { src: chatgpt }],
    [agents.grok.id, { src: grok }],
    [agents.muse.id, { src: muse, light: true }],
    [agents.hermes.id, { src: hermes, light: true }],
    [agents.openclaw.id, { src: openclaw }],
    [agents.scout.id, { glyph: 'scout' }],
    [builtinAgent.id, { glyph: 'workspace' }],
  ])
  return marks.get(seed)
}
