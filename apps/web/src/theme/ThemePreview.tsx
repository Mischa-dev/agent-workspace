import type { CSSProperties } from 'react'

import { ActorTag, Avatar, Badge, Dots, Icon, formatTime } from '../ui'

/** A built-in theme in miniature: rail, sidebar and content in its colors. */
export function PresetMini({ id }: { id: string }) {
  return (
    <span aria-hidden="true" className="aw-mini" data-theme={id}>
      <span className="aw-mini__rail" />
      <span className="aw-mini__side">
        <i />
        <i />
        <i />
      </span>
      <span className="aw-mini__main">
        <i />
        <i />
        <b />
      </span>
    </span>
  )
}

/** A time today, as the clock on this device shows it: "10:24 AM" or "10:24". */
function todayAt(hours: number, minutes: number) {
  const date = new Date()
  date.setHours(hours, minutes, 0, 0)
  return formatTime(date.toISOString())
}

/** A small, real rendering of the shell so the sidebar and rail colors show on this page too. */
export function ThemePreview({ name, seed }: { name: string; seed: string }) {
  return (
    <div aria-hidden="true" className="aw-preview">
      <div className="aw-preview__rail" data-region="rail">
        <span className="aw-tile aw-tile--utility is-active">
          <span className="aw-tile__indicator" />
          <span className="aw-tile__face">
            <Icon name="message" size={16} />
          </span>
        </span>
        <span className="aw-tile aw-tile--space is-unread">
          <span className="aw-tile__indicator" />
          <span className="aw-tile__face" style={{ '--tile-bg': 'var(--swatch-indigo)' } as CSSProperties}>
            AW
          </span>
          <Badge className="aw-tile__badge" count={2} />
        </span>
        <span className="aw-tile aw-tile--space">
          <span className="aw-tile__face" style={{ '--tile-bg': 'var(--swatch-teal)' } as CSSProperties}>
            HL
          </span>
        </span>
      </div>
      <div className="aw-preview__side" data-region="sidebar">
        <div className="aw-preview__title">Agent Workspace</div>
        <span className="aw-item">
          <span className="aw-item__lead">
            <Icon name="hash" />
          </span>
          <span className="aw-item__label">general</span>
        </span>
        <span className="aw-item is-active">
          <span className="aw-item__lead">
            <Icon name="hash" />
          </span>
          <span className="aw-item__label">design</span>
        </span>
        <span className="aw-item is-unread">
          <span className="aw-item__lead">
            <Icon name="hash" />
          </span>
          <span className="aw-item__label">deploys</span>
          <span className="aw-item__trail">
            <Badge count={3} />
          </span>
        </span>
        <span className="aw-item">
          <span className="aw-item__lead">
            <Avatar kind="agent" name="Muse" size={20} />
          </span>
          <span className="aw-item__label">Muse</span>
          <span className="aw-item__trail">
            <Dots />
          </span>
        </span>
      </div>
      <div className="aw-preview__main" data-region="content">
        <div className="aw-preview__msg">
          <Avatar name={name} seed={seed} size={28} />
          <div>
            <div className="aw-msg__meta">
              <span className="aw-msg__author">{name}</span>
              <span className="aw-msg__time">{todayAt(10, 24)}</span>
            </div>
            <p className="aw-preview__text">
              <span className="aw-mention">@Muse</span> can you check the new colors? <a href="#preview">Link</a>
            </p>
          </div>
        </div>
        <div className="aw-preview__msg is-highlighted">
          <Avatar kind="agent" name="Muse" size={28} />
          <div>
            <div className="aw-msg__meta">
              <span className="aw-msg__author">Muse</span>
              <ActorTag kind="agent" />
              <span className="aw-msg__time">{todayAt(10, 27)}</span>
            </div>
            <p className="aw-preview__text">
              Looks good. <span className="aw-mention is-you">@you</span>, the unread badge reads clearly.
            </p>
          </div>
        </div>
        <div className="aw-preview__composer">
          <span>Message #design</span>
          <span className="aw-composer__send">
            <Icon name="send" />
          </span>
        </div>
      </div>
    </div>
  )
}
