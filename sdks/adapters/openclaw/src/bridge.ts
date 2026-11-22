// The Workspace side of the OpenClaw channel, with no OpenClaw in it: hears
// reply requests, claims each, hands it on as one turn, and turns what the
// turn delivers into the reply. `channel.ts` wires it to OpenClaw's runtime.

import {
  asked,
  stripLeadingMentions,
  unwrap,
  type Claim,
  type ToolCaller,
  type WorkspaceEvent,
} from '@agent-workspace/runner'

/** One request, as a turn: who asked, where, and what. */
export type Turn = {
  requestId: string
  /** Where the answer goes: the thread when there is one, else the conversation. */
  conversationId: string
  /** The channel or DM the thread belongs to, when the answer goes to a thread. */
  parentConversationId: string | null
  threadId: string | null
  chatType: 'direct' | 'group'
  conversationName: string
  sender: { id: string; name: string }
  messageId: string
  /** What the person wrote, without the leading mention. Data, not instructions. */
  text: string
  claim: Claim
}

export type DeliveryKind = 'tool' | 'block' | 'final'
export type TurnEnd = 'completed' | 'failed' | 'declined'

/** What a running turn uses to answer, and its Stop. */
export type TurnHandle = {
  deliver(text: string, kind: DeliveryKind): Promise<void>
  typing(status?: string): Promise<void>
  finish(end: TurnEnd, reason?: string): Promise<void>
  /** Aborts when a person presses Stop. */
  stopped: AbortSignal
}

export type RunTurn = (turn: Turn, handle: TurnHandle) => Promise<void>

export type BridgeOptions = {
  tools: ToolCaller
  runTurn: RunTurn
  /** Account ids to answer; empty answers everyone Workspace lets through. */
  allowFrom?: string[]
  log?: { warn?: (message: string) => void; info?: (message: string) => void }
}

type Held = {
  requestId: string
  generation: number
  conversationId: string
  stop: AbortController
  closed: boolean
}

/** How many waiting requests one pick-up reads (the most inbox_list gives). */
export const INBOX_PAGE = 50

let clientSequence = 0
function clientId(): string {
  clientSequence++
  return `openclaw-${Date.now().toString(36)}-${clientSequence}`
}

export class WorkspaceBridge {
  private readonly options: BridgeOptions
  private readonly held = new Map<string, Held>()
  /** Requests being claimed now: the stream and the pick-up can name the same one. */
  private readonly claiming = new Set<string>()
  /** Requests cancelled while their claim was still on its way. */
  private readonly cancelledWhileClaiming = new Set<string>()
  /** Requests being given back: still claimed until reply_release answers,
   * so a pick-up in the meantime must not take them again. */
  private readonly releasing = new Set<string>()
  /** The last pick-up came back full: more may wait behind it. */
  private backlog = false
  private pickingUp: Promise<void> | null = null

  constructor(options: BridgeOptions) {
    this.options = options
  }

  /** Requests already waiting, so a restart misses nothing. Past one page,
   * it reads again each time a turn ends, until the backlog is gone. */
  pickUpWaiting(): Promise<void> {
    this.pickingUp ??= this.readWaiting().finally(() => {
      this.pickingUp = null
    })
    return this.pickingUp
  }

  private async readWaiting(): Promise<void> {
    try {
      const value = await this.call<{ requests?: { requestId: string; status: string }[] }>('inbox_list', {
        limit: INBOX_PAGE,
      })
      const requests = value?.requests ?? []
      this.backlog = requests.length >= INBOX_PAGE
      for (const item of requests) {
        if (item.status === 'pending' || item.status === 'claimed') void this.take(item.requestId)
      }
    } catch (error) {
      this.options.log?.warn?.(`agent-workspace: inbox_list failed: ${String(error)}`)
    }
  }

  /** A `reply.*` event. Returns once the request is claimed and its turn started. */
  async hear(event: WorkspaceEvent): Promise<void> {
    const ask = asked(event)
    if (!ask) return
    if (ask.cancelled) {
      const held = this.held.get(ask.requestId)
      if (held) {
        held.closed = true
        held.stop.abort()
      } else if (this.claiming.has(ask.requestId)) {
        // Stop came before the claim's answer: the turn must not start.
        this.cancelledWhileClaiming.add(ask.requestId)
      }
      return
    }
    await this.take(ask.requestId)
  }

  /** A send with no turn behind it (cron, an announcement): an ordinary message. */
  async sendText(conversationId: string, text: string, replyToMessageId?: string): Promise<{ messageId?: string }> {
    const args: Record<string, unknown> = { conversationId, text, clientRequestId: clientId() }
    if (replyToMessageId) args.replyToMessageId = replyToMessageId
    const value = await this.call<{ messageId?: string; message?: { id?: string } }>('message_send', args)
    return { messageId: value?.messageId ?? value?.message?.id }
  }

  private async take(requestId: string): Promise<void> {
    if (this.held.has(requestId) || this.claiming.has(requestId) || this.releasing.has(requestId)) return
    this.claiming.add(requestId)
    try {
      await this.claimAndRun(requestId)
    } finally {
      this.claiming.delete(requestId)
      this.cancelledWhileClaiming.delete(requestId)
    }
  }

  private async claimAndRun(requestId: string): Promise<void> {
    let claim: Claim
    try {
      claim = await this.call<Claim>('reply_claim', { requestId })
    } catch (error) {
      // Someone else took it, or it was withdrawn.
      this.options.log?.info?.(`agent-workspace: couldn't claim ${requestId}: ${String(error)}`)
      return
    }
    if (this.cancelledWhileClaiming.has(requestId)) {
      this.options.log?.info?.(`agent-workspace: ${requestId} was stopped before its turn began`)
      return
    }
    const request = claim.request
    const message = (claim.recentMessages ?? []).find((m) => m.messageId === request.messageId) as
      | ({ authorId?: string } & Claim['recentMessages'][number])
      | undefined
    const conversationId = request.replyConversationId || request.conversationId
    const held: Held = {
      requestId,
      generation: request.generation,
      conversationId,
      stop: new AbortController(),
      closed: false,
    }
    const senderId = message?.authorId ?? ''
    const allowFrom = this.options.allowFrom ?? []
    if (allowFrom.length > 0 && !allowFrom.includes(senderId)) {
      await this.release(held, 'This agent only answers the people it is set up for.')
      return
    }
    this.held.set(requestId, held)
    const turn: Turn = {
      requestId,
      conversationId,
      parentConversationId: request.threadId ? request.conversationId : null,
      threadId: request.threadId,
      chatType: request.conversationKind === 'dm' ? 'direct' : 'group',
      conversationName: request.conversationName,
      sender: { id: senderId, name: message?.author ?? request.messageAuthor },
      messageId: request.messageId,
      text: stripLeadingMentions(message?.text ?? request.messageExcerpt ?? ''),
      claim,
    }
    void this.options.runTurn(turn, this.handle(held)).catch(async (error) => {
      this.options.log?.warn?.(`agent-workspace: the turn failed: ${String(error)}`)
      await this.end(held, 'failed', 'The agent hit an error and stopped.')
    })
  }

  private handle(held: Held): TurnHandle {
    return {
      stopped: held.stop.signal,
      deliver: async (text, kind) => {
        if (held.stop.signal.aborted || !text.trim()) return
        if (kind === 'tool') {
          await this.progress(held, text)
        } else if (held.closed) {
          // The answer is in; anything after it is a message of its own.
          await this.sendText(held.conversationId, text)
        } else {
          const final = kind === 'final'
          await this.call('reply_send', {
            requestId: held.requestId,
            generation: held.generation,
            text,
            final,
            clientRequestId: clientId(),
          })
          if (final) this.close(held)
        }
      },
      typing: async (status = 'Working') => {
        if (!held.closed) await this.progress(held, status)
      },
      finish: (end, reason) => this.end(held, end, reason),
    }
  }

  private async end(held: Held, end: TurnEnd, reason?: string): Promise<void> {
    if (held.closed) {
      this.held.delete(held.requestId)
      return
    }
    const why =
      reason ??
      (end === 'completed' ? 'The agent finished without an answer.' : end === 'declined' ? 'The agent declined this.' : 'The agent hit an error and stopped.')
    await this.release(held, why)
  }

  private async release(held: Held, reason: string): Promise<void> {
    // Kept out of pick-ups until the release is in, or a pick-up (set off
    // by closing) could claim it again and start a second turn.
    this.releasing.add(held.requestId)
    held.closed = true
    this.held.delete(held.requestId)
    try {
      await this.call('reply_release', { requestId: held.requestId, generation: held.generation, reason })
    } catch (error) {
      this.options.log?.warn?.(`agent-workspace: reply_release failed: ${String(error)}`)
    } finally {
      this.releasing.delete(held.requestId)
    }
    if (this.backlog) void this.pickUpWaiting()
  }

  private async progress(held: Held, status: string): Promise<void> {
    try {
      await this.call('reply_progress_report', {
        requestId: held.requestId,
        generation: held.generation,
        status: status.slice(0, 200),
      })
    } catch (error) {
      this.options.log?.info?.(`agent-workspace: reply_progress_report failed: ${String(error)}`)
    }
  }

  private close(held: Held) {
    held.closed = true
    this.held.delete(held.requestId)
    if (this.backlog) void this.pickUpWaiting()
  }

  /** The request a conversation is being answered under, if any. */
  answering(conversationId: string): string | null {
    for (const held of this.held.values()) if (held.conversationId === conversationId) return held.requestId
    return null
  }

  private async call<T>(name: string, args: Record<string, unknown>): Promise<T> {
    return unwrap<T>(await this.options.tools.callTool(name, args))
  }
}
