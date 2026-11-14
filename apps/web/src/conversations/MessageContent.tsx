import type { ReactNode } from 'react'
import { Link } from 'react-router-dom'

import { thingLink } from '../modules/references/thingLinks'
import { Markdown } from '../ui/markdown/Markdown'
import { parseMessageContent } from './messageContentModel'
import type { ConversationActor, ConversationWorkItemChip } from './types'
import './messageMarkdown.css'
import '../styles/conversation.css'

type MessageContentProps = {
  actors: ConversationActor[]
  content: unknown
  fallbackPlainText: string
  workItems?: ConversationWorkItemChip[]
  /** Mentions of this actor are marked as mentions of you. */
  viewerId?: string
}

function workItemState(statusCategory: string) {
  if (statusCategory === 'done' || statusCategory === 'completed' || statusCategory === 'canceled') return 'done'
  if (statusCategory === 'in_progress' || statusCategory === 'started' || statusCategory === 'active') return 'doing'
  return 'todo'
}

/** `@everyone` or `@here` as a word, which notify everyone who can see the conversation. */
const EVERYONE_MENTION = /(?<![\p{L}\p{N}_@])@(?:everyone|here)(?![\p{L}\p{N}_])/giu

/** Text with `@everyone` and `@here` marked like mentions. */
function TextWithEveryone({ text }: { text: string }) {
  const parts: Array<string | { mention: string }> = []
  let last = 0
  for (const match of text.matchAll(EVERYONE_MENTION)) {
    const start = match.index
    if (start > last) parts.push(text.slice(last, start))
    parts.push({ mention: match[0] })
    last = start + match[0].length
  }
  if (parts.length === 0) return <>{text}</>
  if (last < text.length) parts.push(text.slice(last))
  return (
    <>
      {parts.map((part, index) =>
        typeof part === 'string' ? (
          part
        ) : (
          <span className="aw-mention is-everyone" key={index} title="Notifies everyone who can see this conversation">
            {part.mention}
          </span>
        ),
      )}
    </>
  )
}

/** Placeholders for chips inside the Markdown source: private-use characters around the node's index. */
const OPEN = '\uE000'
const CLOSE = '\uE001'
const PLACEHOLDER = /\uE000(\d+)\uE001/g

/**
 * Message text as Markdown (lists, quotes, code, links; never HTML), with
 * people, agents and tasks as chips where they were picked.
 */
export function MessageContent({ actors, content, fallbackPlainText, workItems = [], viewerId }: MessageContentProps) {
  const parsed = parseMessageContent(content)
  // Text nodes can't hold the private-use placeholders themselves: the
  // server refuses control characters but not these, so drop any typed in.
  const source = parsed
    ? parsed.nodes
        .map((node, index) => (node.type === 'text' ? node.text.replace(/[\uE000\uE001]/g, '') : `${OPEN}${index}${CLOSE}`))
        .join('')
    : null
  if (!parsed || source === null) {
    return <span className="aw-content">{fallbackPlainText}</span>
  }

  const actorsById = new Map(actors.map((actor) => [actor.id, actor]))
  const workItemsById = new Map(workItems.map((item) => [item.id, item]))

  const chip = (index: number): ReactNode => {
    const node = parsed.nodes[index]
    if (!node || node.type === 'text') return null
    if (node.type === 'work_item_ref') {
      const item = workItemsById.get(node.workItemId)
      if (!item) {
        return (
          <span aria-label="A task you can’t see" className="aw-workref is-unavailable" key={`chip-${index}`} title="A task you can’t see">
            Unavailable task
          </span>
        )
      }
      return (
        <Link
          aria-label={item.title ? `${item.key}: ${item.title}` : item.key}
          className="aw-workref"
          data-archived={Boolean(item.archivedAt)}
          key={`chip-${index}`}
          title={item.title ?? item.key}
          to={item.href}
        >
          {item.statusCategory ? (
            <span aria-hidden="true" className={`aw-workref__status is-${workItemState(item.statusCategory)}`} />
          ) : null}
          <span className="aw-workref__key">{item.key}</span>
          {item.title ? <span className="aw-workref__title">{item.title}</span> : null}
        </Link>
      )
    }
    const actor = actorsById.get(node.actorId)
    if (!actor) {
      return (
        <span
          aria-label="Someone who isn’t here any more"
          className="aw-mention is-unavailable"
          key={`chip-${index}`}
          title="Someone who isn’t here any more, or whom you can’t see"
        >
          @someone
        </span>
      )
    }
    return (
      <span
        aria-label={`${actor.displayName}, @${actor.handle}`}
        className={actor.id === viewerId ? 'aw-mention is-you' : 'aw-mention'}
        data-actor-id={actor.id}
        key={`chip-${index}`}
        title={`@${actor.handle}`}
      >
        @{actor.displayName}
      </span>
    )
  }

  /** What a chip reads as inside code, where only text can go. */
  const chipText = (index: number) => {
    const node = parsed.nodes[index]
    if (node?.type === 'mention') {
      const actor = actorsById.get(node.actorId)
      return actor ? `@${actor.handle}` : '@someone'
    }
    if (node?.type === 'work_item_ref') return workItemsById.get(node.workItemId)?.key ?? ''
    return ''
  }

  const renderText = (value: string): ReactNode => {
    if (!value.includes(OPEN)) return <TextWithEveryone text={value} />
    const parts: ReactNode[] = []
    let last = 0
    for (const match of value.matchAll(PLACEHOLDER)) {
      if (match.index > last) parts.push(<TextWithEveryone key={`text-${last}`} text={value.slice(last, match.index)} />)
      parts.push(chip(Number(match[1])))
      last = match.index + match[0].length
    }
    if (last < value.length) parts.push(<TextWithEveryone key={`text-${last}`} text={value.slice(last)} />)
    return parts
  }

  const codeText = (value: string) => value.replace(PLACEHOLDER, (_, index: string) => chipText(Number(index)))

  return <Markdown className="aw-content" codeText={codeText} renderLink={thingLink} renderText={renderText} source={source} />
}
