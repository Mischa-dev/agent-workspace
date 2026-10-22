import type { CSSProperties } from 'react'

import { Icon } from '../../../src/ui'
import { agents, message, museMock, people, type Actor } from '../data'
import { clamp, ease, mix, progress, typed } from '../motion'
import {
  Arrive,
  ChannelHeader,
  Composer,
  Day,
  MentionList,
  Message,
  Reactions,
  Replies,
  ThreadLine,
  ThreadPanel,
  Timeline,
  WritingRow,
  reaction,
  request,
} from '../parts/Chat'
import { AppWindow, ProjectSidebar, Shell, agentChange } from '../parts/Shell'
import { Swap } from '../parts/Swap'
import { Caption } from '../parts/Text'

/*
 * 0:34. #launch: an agent's morning update, teammates talking, another agent
 * chiming in with a heads-up. Then Priya tags two agents, and they answer in a
 * thread that opens beside the channel.
 */

const WIN = { w: 1260, h: 940, x: 600, y: 70, k: 1 }
// Once the thread is open: the window widens by the thread's column, so the channel keeps its width.
const WIDE = { x: 204, y: 40, k: 0.9 }
const THREAD = 420

const T = {
  claude: 0.5,
  tada: 1.25,
  tada2: 2.0,
  leo: 1.7,
  rocket: 2.3,
  priya: 2.65,
  hermes: 3.55,
  thanks: 4.4,
  sam: 4.85,
  hermesWorking: 5.35,
  focus: 5.65,
  at1: 5.95,
  query1: 6.15,
  pick1: 6.5,
  text1: 6.55,
  at2: 7.3,
  query2: 7.45,
  pick2: 7.75,
  text2: 7.8,
  send: 8.95,
  claimed: 9.35,
  open: 9.8,
  codex: 10.35,
  codexDone: 11.75,
  muse: 12.2,
  leoThread: 13.45,
  heart: 14.05,
  exit: 15.45,
}

const ONE = 'can you take NW-12? '
const TWO = 'could you mock the Export button in our colors?'
const TYPED_ONE = 1 / 32
const TYPED_TWO = 1 / 44

const MEMBERS = [people.priya, people.leo, people.sam, people.maya, agents.claude, agents.codex, agents.muse, agents.hermes, agents.chatgpt]
const EVERYONE = [agents.claude, agents.codex, agents.chatgpt, agents.grok, agents.muse, agents.hermes, agents.openclaw, people.leo]

const M = {
  sam1: message(14, people.sam, '11:02', ['Morning all! The offsite agenda is pinned at the top of #general.']),
  maya1: message(15, people.maya, '14:10', ['Design review for the board moved to 3pm tomorrow. Bring your notes on the done column.']),
  claw1: message(16, agents.openclaw, '14:32', ['Nightly backups now include uploaded files. I restored last night’s copy to check it.']),
  leo1: message(17, people.leo, '15:05', ['Nice, that was the last thing blocking the alpha.']),
  sam0: message(1, people.sam, '15:58', ['The demo with Acme went well. They loved the board, and asked when export lands.']),
  leo0: message(2, people.leo, '16:32', ['Pushed the new column order to ', { task: 9, state: 'completed' }, '.']),
  gpt0: message(3, agents.chatgpt, '17:05', [
    'The Markdown importer is ready for review in ',
    { task: 19, state: 'review' },
    '. Nested lists become subtasks, and checked items arrive done.',
  ]),
  maya0: message(4, people.maya, '08:41', ['Release notes for 1.4 are drafted in Pages. Comments welcome before Friday.']),
  claude: message(5, agents.claude, '09:02', [
    'Morning! Overnight I fixed ',
    { task: 18, state: 'review' },
    ' and opened PR #412. Tests pass. Next I’ll start on ',
    { task: 16, state: 'backlog' },
    ' unless someone needs me elsewhere.',
  ]),
  leo: message(6, people.leo, '09:05', ['Great work ', agents.claude, '. ', people.priya, ' do the export columns look right to you?']),
  priya: message(7, people.priya, '09:06', ['Yes, it’s the order we agreed in ', { task: 9, state: 'completed' }, '.']),
  hermes: message(8, agents.hermes, '09:06', [
    'Heads up for ',
    { task: 12, state: 'unstarted' },
    ': big projects time out if the export loads every row at once. Streaming the rows fixes it. I can write a load test with 10,000 tasks if that helps.',
  ]),
  sam: message(9, people.sam, '09:07', ['Yes please, ', agents.hermes, '!']),
  ask: message(10, people.priya, '09:08', [agents.codex, ' can you take ', { task: 12, state: 'unstarted' }, '? ', agents.muse, ' could you mock the Export button in our colors?']),
  codex: message(11, agents.codex, '09:08', [
    'On it. I’ll start with ',
    { task: 26, state: 'unstarted' },
    ' and stream the rows, as Hermes suggested. I’ll open a pull request when the tests pass.',
  ]),
  muse: message(12, agents.muse, '09:09', ['Here’s a first pass: a secondary button next to Filter, so it doesn’t compete with New task.'], {
    attachments: [museMock],
  }),
  leoThread: message(13, people.leo, '09:10', ['Love it. Ship it!']),
}

const CODEX_TEXT = 'On it. I’ll start with NW-26 and stream the rows, as Hermes suggested. I’ll open a pull request when the tests pass.'

type Mentions = { query: string; k: number; out: number; active: string; was: string; moved: number; rows: { actor: Actor; k: number; out?: number }[] }

// Everyone the list can show: the channel's agents and Leo, and Scout, who only comes up for "co".
const MENTIONABLE = [...EVERYONE, agents.scout]

/** The mention list from `@` at `at`: it narrows to `matches` as the query starts, and closes as one is picked. */
function mentions(t: number, at: number, queryAt: number, pickAt: number, query: string, matches: Actor[]): Mentions {
  const narrow = progress(t, queryAt, 0.26)
  return {
    query,
    k: progress(t, at, 0.18),
    out: progress(t, pickAt, 0.12),
    active: matches[0].id,
    was: EVERYONE[0].id,
    moved: progress(t, queryAt, 0.16),
    rows: MENTIONABLE.map((actor) => {
      const listed = EVERYONE.includes(actor)
      if (matches.includes(actor)) return { actor, k: listed ? 1 : narrow }
      return { actor, k: listed ? 1 : 0, out: narrow }
    }),
  }
}

// Keys a second while a mention's query is typed; the list narrows from the first one.
const KEYS = 12

/** What's in the composer at t, and the mention list while one is open. */
function draft(t: number): { text: string; typing: boolean; list?: Mentions } {
  if (t < T.at1) return { text: '', typing: false }
  const first = `@Codex ${typed(ONE, t, T.text1, 1 / TYPED_ONE)}`
  if (t < T.pick1 + 0.1) {
    const query = typed('co', t, T.query1, KEYS)
    return { text: t < T.pick1 ? `@${query}` : first, typing: true, list: mentions(t, T.at1, T.query1 + 1 / KEYS, T.pick1, query, [agents.codex, agents.scout]) }
  }
  if (t < T.at2) return { text: first, typing: t < T.text1 + ONE.length * TYPED_ONE + 0.1 }
  if (t < T.pick2 + 0.1) {
    const query = typed('mu', t, T.query2, KEYS)
    return { text: t < T.pick2 ? `${first}@${query}` : `${first}@Muse `, typing: true, list: mentions(t, T.at2, T.query2 + 1 / KEYS, T.pick2, query, [agents.muse]) }
  }
  const second = `${first}@Muse ${typed(TWO, t, T.text2, 1 / TYPED_TWO)}`
  if (t < T.send + 0.08) return { text: second, typing: t < T.text2 + TWO.length * TYPED_TWO + 0.1 }
  return { text: '', typing: false }
}

export function Team({ t }: { t: number }) {
  const k = (at: number, length = 0.45) => progress(t, at, length)
  const open = progress(t, T.open, 0.75, ease.inOut)
  const width = WIN.w + THREAD * open
  const scale = mix(WIN.k, WIDE.k, open)
  const arrive = progress(t, 0, 0.55)
  const windowStyle: CSSProperties = {
    left: 0,
    top: 0,
    width,
    height: WIN.h,
    transformOrigin: '0 0',
    opacity: arrive * (1 - progress(t, T.exit, 0.4, ease.inOut)),
    transform: `translate3d(${mix(WIN.x, WIDE.x, open) + (1 - arrive) * 30}px, ${mix(WIN.y, WIDE.y, open)}px, 0) scale(${scale * mix(0.97, 1, arrive)})`,
  }

  const composer = draft(t)
  const sent = t >= T.send + 0.05
  const codexReplied = t >= T.codexDone
  const museReplied = t >= T.muse
  const replies = (museReplied ? 1 : 0) + (codexReplied ? 1 : 0) + (t >= T.leoThread ? 1 : 0)

  const sidebar = (
    <ProjectSidebar
      active="#launch"
      agents={[
        agentChange(t, T.claimed, agents.codex, { state: 'idle', status: 'Ready for work' }, { state: 'working', status: 'Working on NW-12' }),
        { actor: agents.claude, state: 'working', status: 'Starting NW-16' },
        agentChange(t, T.claimed + 0.2, agents.muse, { state: 'working', status: 'Designing NW-14' }, { state: 'working', status: 'Mocking the Export button' }),
        agentChange(t, T.hermesWorking, agents.hermes, { state: 'idle', status: 'Ready for work' }, { state: 'working', status: 'Load testing NW-12' }),
      ]}
      channels={[{ name: 'general' }, { name: 'launch' }, { name: 'design', unread: true }]}
      dms={[{ actor: people.leo }, { actor: agents.grok, working: true }]}
    />
  )

  const thread =
    open > 0 ? (
      <div style={{ height: '100%', opacity: clamp(open * 1.6) }}>
        <ThreadPanel name="Export tasks as CSV" origin={<Message entry={M.ask} />}>
          <Arrive id="codex-thread" k={k(T.codex, 0.35)}>
            <Swap
              from={<WritingRow actor={agents.codex} k={(t - T.codex) / (T.codexDone - T.codex - 0.1)} text={CODEX_TEXT} />}
              id="codex-thread"
              k={progress(t, T.codexDone, 0.3)}
              to={<Message entry={M.codex} />}
            />
          </Arrive>
          <Arrive id="muse" k={k(T.muse)}>
            <Message
              entry={M.muse}
              extras={<Reactions id="muse" k={k(T.heart, 0.3)} reactions={[reaction('❤️', 'heart', [people.leo, people.priya], true)]} />}
            />
          </Arrive>
          <Arrive id="leo-thread" k={k(T.leoThread)}>
            <Message entry={M.leoThread} />
          </Arrive>
        </ThreadPanel>
      </div>
    ) : null

  return (
    <div className="tr-layer">
      <Caption
        at={0.3}
        exitAt={T.focus - 0.2}
        exitTo={{ x: -90 }}
        kicker="Teammates"
        kickerIcon={<Icon name="users" size={26} />}
        sub="Agents post updates and chime in when they can help, like any teammate."
        t={t}
        title={['Your team.', 'And your', '*agents.*']}
        titleScale={2.6}
        width={440}
        x={110}
        y={290}
      />
      <Caption
        at={T.focus + 0.1}
        exitAt={T.open - 0.3}
        exitTo={{ x: -90 }}
        sub="Claude, Codex, ChatGPT, Grok, Muse, Hermes, OpenClaw, or your own."
        t={t}
        title={['Tag any', '*agent.*']}
        titleScale={2.9}
        width={440}
        x={110}
        y={340}
      />
      <Caption
        align="center"
        at={T.open + 0.45}
        exitAt={T.exit}
        t={t}
        title={['They answer in a thread, and show you what they’re doing.']}
        titleScale={1.45}
        width={1600}
        x={160}
        y={912}
      />
      <div className="tr-abs" style={windowStyle}>
        <AppWindow height={WIN.h} width={width}>
          <Shell context={thread} contextWidth={open > 0 ? THREAD * open : undefined} main="conversation" sidebar={sidebar}>
            <ChannelHeader members={MEMBERS} name="launch" topic="Shipping 1.4 on Nov 14" />
            <Timeline>
              <Day label="Monday, October 19" />
              <Message entry={M.sam1} />
              <Day label="Yesterday" />
              <Message entry={M.maya1} />
              <Message entry={M.claw1} reactions={[reaction('✅', 'check', [people.leo, people.sam])]} />
              <Message entry={M.leo1} />
              <Message entry={M.sam0} />
              <Message entry={M.leo0} reactions={[reaction('👍', 'thumbsup', [people.sam, people.maya])]} />
              <Message entry={M.gpt0} />
              <Day label="Today" />
              <Message entry={M.maya0} reactions={[reaction('👀', 'eyes', [people.leo])]} />
              <Arrive id="claude" k={k(T.claude)}>
                <Message
                  entry={M.claude}
                  extras={
                    <Reactions
                      id="claude"
                      k={k(T.tada, 0.3)}
                      pops={[
                        { index: 0, k: progress(t, T.tada2, 0.3), kind: 'bump' },
                        { index: 1, k: progress(t, T.rocket, 0.3), kind: 'join' },
                      ]}
                      reactions={[
                        reaction('🎉', 'tada', t >= T.tada2 ? [people.leo, people.sam, people.maya] : [people.leo, people.sam]),
                        ...(t >= T.rocket ? [reaction('🚀', 'rocket', [people.priya], true)] : []),
                      ]}
                    />
                  }
                />
              </Arrive>
              <Arrive id="leo" k={k(T.leo)}>
                <Message entry={M.leo} />
              </Arrive>
              <Arrive id="priya" k={k(T.priya)}>
                <Message entry={M.priya} />
              </Arrive>
              <Arrive id="hermes" k={k(T.hermes)}>
                <Message
                  entry={M.hermes}
                  extras={
                    <Reactions
                      id="hermes"
                      k={k(T.thanks, 0.3)}
                      reactions={[reaction('🙏', 'pray', [people.sam]), reaction('👍', 'thumbsup', [people.leo, people.maya])]}
                    />
                  }
                />
              </Arrive>
              <Arrive id="sam" k={k(T.sam)}>
                <Message
                  entry={M.sam}
                  extras={
                    <Arrive id="sam-replies" k={k(T.hermesWorking, 0.3)}>
                      <Replies requests={[request('hermes-load', agents.hermes, 'claimed', 'Writing a load test for NW-12')]} />
                    </Arrive>
                  }
                />
              </Arrive>
              <Arrive id="ask" k={sent ? k(T.send + 0.05, 0.4) : 0}>
                <Message
                  entry={M.ask}
                  extras={
                    <>
                      <Arrive id="ask-codex" k={k(T.claimed, 0.3)} out={progress(t, T.codexDone, 0.3)}>
                        <Replies requests={[request('codex-ask', agents.codex, 'claimed', 'Reading NW-12 and its subtasks')]} />
                      </Arrive>
                      <Arrive id="ask-muse" k={k(T.claimed + 0.2, 0.3)} out={progress(t, T.muse, 0.3)}>
                        <Replies requests={[request('muse-ask', agents.muse, 'claimed')]} />
                      </Arrive>
                      <Arrive id="ask-thread" k={k(T.claimed + 0.1, 0.3)}>
                        <ThreadLine current={open > 0.5} name="Export tasks as CSV" replies={replies} working />
                      </Arrive>
                    </>
                  }
                />
              </Arrive>
            </Timeline>
            <Composer
              caret={t >= T.focus}
              list={composer.list ? <MentionList {...composer.list} /> : undefined}
              placeholder="Message #launch"
              sending={t >= T.send && t < T.send + 0.16}
              text={composer.text}
              typing={composer.typing}
            />
          </Shell>
        </AppWindow>
      </div>
    </div>
  )
}
