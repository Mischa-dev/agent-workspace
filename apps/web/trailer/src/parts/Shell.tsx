import type { CSSProperties, ReactNode } from 'react'

import { Avatar, Badge, Dots, Icon, type IconName } from '../../../src/ui'
import type { Actor } from '../data'
import { project, viewer } from '../data'
import { clamp } from '../motion'

/*
 * The app frame, drawn with the app's own shell classes (shell.css) and kit
 * components, so it is the real rail, sidebar and main column at any size.
 */

export function AppWindow({
  children,
  height,
  style,
  theme = 'indigo',
  width,
}: {
  children: ReactNode
  height: number
  style?: CSSProperties
  theme?: string
  width: number
}) {
  return (
    <div className="tr-window" data-theme={theme} style={{ width, height, ...style }}>
      {children}
    </div>
  )
}

export function Shell({
  children,
  context,
  contextWidth,
  main = 'page',
  sidebar,
}: {
  children: ReactNode
  /** The column on the right (a thread beside a channel), in the app's own panel. */
  context?: ReactNode
  /** How far the context column has opened, in pixels, while it slides in. */
  contextWidth?: number
  main?: 'page' | 'conversation'
  sidebar?: ReactNode
}) {
  const sliding = context && contextWidth !== undefined
  return (
    <div
      className="aw-shell tr-shell"
      data-has-context={context ? 'true' : undefined}
      data-has-sidebar={sidebar ? 'true' : undefined}
      style={sliding ? { gridTemplateColumns: `var(--size-rail) var(--size-sidebar) minmax(0, 1fr) ${contextWidth}px` } : undefined}
    >
      <Rail />
      {sidebar}
      <main className={`aw-main aw-main--${main} tr-main`}>{children}</main>
      {context ? (
        <div className="aw-panel" data-region="content">
          {context}
        </div>
      ) : null}
    </div>
  )
}

function Tile({
  active,
  badge,
  children,
  quiet,
  swatch,
}: {
  active?: boolean
  badge?: number
  children: ReactNode
  quiet?: boolean
  swatch?: string
}) {
  const kind = swatch ? 'aw-tile--space' : 'aw-tile--utility'
  return (
    <span className={['aw-tile', kind, quiet ? 'aw-tile--quiet' : '', active ? 'is-active' : '', badge ? 'is-unread' : ''].filter(Boolean).join(' ')}>
      <span aria-hidden="true" className="aw-tile__indicator" />
      <span className="aw-tile__face" style={swatch ? ({ '--tile-bg': `var(--swatch-${swatch})` } as CSSProperties) : undefined}>
        {children}
      </span>
      {badge ? <Badge className="aw-tile__badge" count={badge} max={9} /> : null}
    </span>
  )
}

export function Rail({ home = false, inbox = 3 }: { home?: boolean; inbox?: number }) {
  return (
    <nav aria-label="Spaces" className="aw-rail" data-region="rail">
      <Tile active={home} badge={inbox}>
        <Icon name="home" size={20} />
      </Tile>
      <Tile badge={2}>
        <Icon name="message" size={20} />
      </Tile>
      <span className="aw-rail__divider" />
      <div className="aw-rail__projects">
        <Tile active={!home} swatch={project.swatch}>
          {project.key}
        </Tile>
        <Tile swatch="blue">AT</Tile>
        <Tile badge={1} swatch="green">
          WE
        </Tile>
      </div>
      <Tile>
        <Icon name="plus" size={20} />
      </Tile>
      <span className="aw-rail__spacer" />
      <Tile quiet>
        <Icon name="search" size={20} />
      </Tile>
      <Tile>
        <Icon name="settings" size={20} />
      </Tile>
      <span className="aw-rail__me">
        <Avatar name={viewer.name} presence="online" seed={viewer.id} size={36} />
      </span>
    </nav>
  )
}

export function SidebarItem({
  active,
  actor,
  badge,
  icon,
  label,
  meta,
  needsYou,
  unread,
  working,
}: {
  active?: boolean
  actor?: Actor
  badge?: number
  icon?: IconName
  label: string
  meta?: string
  needsYou?: boolean
  unread?: boolean
  working?: boolean
}) {
  return (
    <li>
      <span className={['aw-item', active ? 'is-active' : '', unread || badge ? 'is-unread' : ''].filter(Boolean).join(' ')}>
        <span className="aw-item__lead">
          {actor ? <Avatar kind={actor.kind} name={actor.name} seed={actor.id} size={20} /> : icon ? <Icon name={icon} /> : null}
        </span>
        <span className="aw-item__label">{label}</span>
        {working || badge || meta || needsYou ? (
          <span className="aw-item__trail">
            {needsYou ? <span className="aw-attn aw-attn--needs">Needs you</span> : null}
            {working ? <Dots /> : null}
            {badge ? <Badge count={badge} /> : null}
            {meta ? <span className="aw-item__meta">{meta}</span> : null}
          </span>
        ) : null}
      </span>
    </li>
  )
}

type AgentState = 'working' | 'waiting' | 'idle'
export type SidebarAgentEntry = { actor: Actor; state: AgentState; status: string; was?: { state: AgentState; status: string }; k?: number }

/** An agent's sidebar entry changing at `at`: its dot and status line cross-fade from before to after. */
export function agentChange(
  t: number,
  at: number,
  actor: Actor,
  before: { state: AgentState; status: string },
  after: { state: AgentState; status: string },
): SidebarAgentEntry {
  if (t < at) return { actor, ...before }
  return { actor, ...after, was: before, k: clamp((t - at) / 0.35) }
}

export function SidebarAgent({ actor, k = 1, state, status, was }: SidebarAgentEntry) {
  const changing = was && k < 1
  return (
    <li>
      <span className="aw-item aw-sideagent">
        <span className="aw-item__lead aw-sideagent__lead">
          <Avatar kind="agent" name={actor.name} seed={actor.id} size={20} />
          {changing && was.state !== state ? (
            <span aria-hidden className={`aw-agentdot aw-agentdot--${was.state} aw-sideagent__dot`} style={{ opacity: 1 - k }} />
          ) : null}
          <span aria-hidden className={`aw-agentdot aw-agentdot--${state} aw-sideagent__dot`} style={changing && was.state !== state ? { opacity: k } : undefined} />
        </span>
        <span className="aw-item__label aw-sideagent__text">
          <span className="aw-sideagent__name">{actor.name}</span>
          {changing ? (
            <span className="aw-sideagent__state tr-swap">
              <span style={{ opacity: 1 - k, transform: `translate3d(0, ${-6 * k}px, 0)` }}>{was.status}</span>
              <span style={{ opacity: k, transform: `translate3d(0, ${6 * (1 - k)}px, 0)` }}>{status}</span>
            </span>
          ) : (
            <span className="aw-sideagent__state">{status}</span>
          )}
        </span>
      </span>
    </li>
  )
}

export function SidebarSection({ children, label }: { children: ReactNode; label: string }) {
  return (
    <section className="aw-section">
      <div className="aw-section__head">
        <span className="aw-section__toggle">
          <Icon className="aw-section__caret" name="chevron-down" size={14} />
          <span>{label}</span>
        </span>
      </div>
      <ul className="aw-section__list">{children}</ul>
    </section>
  )
}

export function Sidebar({ children, title = project.name }: { children: ReactNode; title?: string }) {
  return (
    <aside className="aw-sidebar" data-region="sidebar">
      <header className="aw-sidebar__header">
        <span className="aw-sidebar__title">
          <span>{title}</span>
          <Icon name="chevron-down" />
        </span>
        <div className="aw-sidebar__actions">
          <span className="aw-iconbtn aw-iconbtn--md">
            <Icon name="search" size={18} />
          </span>
        </div>
      </header>
      <div className="aw-sidebar__scroll">
        <div>{children}</div>
      </div>
    </aside>
  )
}

/** The project's sidebar as the app draws it: its pages, saved views, agents and channels. */
export function ProjectSidebar({
  active,
  agents = [],
  channels = [],
  dms = [],
}: {
  active: string
  agents?: SidebarAgentEntry[]
  channels?: { name: string; unread?: boolean; badge?: number }[]
  dms?: { actor: Actor; unread?: boolean; working?: boolean }[]
}) {
  return (
    <Sidebar>
      <ul className="aw-section__list aw-sidebar__top">
        <SidebarItem active={active === 'overview'} icon="home" label="Overview" />
        <SidebarItem active={active === 'tasks'} icon="work" label="Tasks" />
        <SidebarItem active={active === 'pages'} icon="list" label="Pages" />
        <SidebarItem active={active === 'runs'} icon="activity" label="Runs" />
      </ul>
      <SidebarSection label="Views">
        <SidebarItem active={active === 'my-tasks'} icon="list" label="My tasks" />
        <SidebarItem active={active === 'ready'} icon="bot" label="Ready for agents" meta="6" />
      </SidebarSection>
      {agents.length ? (
        <SidebarSection label="Agents">
          {agents.map((entry) => (
            <SidebarAgent key={entry.actor.id} {...entry} />
          ))}
        </SidebarSection>
      ) : null}
      {channels.length ? (
        <SidebarSection label="Channels">
          {channels.map((channel) => (
            <SidebarItem
              active={active === `#${channel.name}`}
              badge={channel.badge}
              icon="hash"
              key={channel.name}
              label={channel.name}
              unread={channel.unread}
            />
          ))}
        </SidebarSection>
      ) : null}
      {dms.length ? (
        <SidebarSection label="Direct messages">
          {dms.map((dm) => (
            <SidebarItem actor={dm.actor} key={dm.actor.id} label={dm.actor.name} unread={dm.unread} working={dm.working} />
          ))}
        </SidebarSection>
      ) : null}
    </Sidebar>
  )
}
