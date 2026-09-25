import type { ReactNode } from 'react'

import { Icon, Menu, type IconName, type MenuEntry } from '../ui'

/**
 * The bar above a conversation: what you're in, a short subtitle, and tools on the right.
 * The title is the page's h1; with a menu it also opens the conversation's actions.
 */
export function ConversationHeader({
  actions,
  icon,
  lead,
  menu,
  status,
  subtitle,
  tag,
  title,
}: {
  actions?: ReactNode
  icon?: IconName
  /** Replaces the icon, for example with an avatar. */
  lead?: ReactNode
  menu?: MenuEntry[]
  status?: ReactNode
  subtitle?: ReactNode
  tag?: ReactNode
  title: string
}) {
  const face = (
    <>
      {lead ?? (icon ? <Icon className="aw-convhead__glyph" name={icon} size={18} /> : null)}
      <span className="aw-convhead__name">{title}</span>
      {tag}
      {menu?.length ? <Icon className="aw-convhead__caret" name="chevron-down" size={14} /> : null}
    </>
  )
  return (
    <header className="aw-convhead">
      <div className="aw-convhead__main">
        <h1 className="aw-convhead__heading" id="workspace-heading">
          {menu?.length ? (
            <Menu
              items={menu}
              label={`${title} actions`}
              trigger={
                <button className="aw-convhead__title" type="button">
                  {face}
                </button>
              }
            />
          ) : (
            <span className="aw-convhead__title aw-convhead__title--static">{face}</span>
          )}
        </h1>
        {subtitle ? <div className="aw-convhead__topic">{subtitle}</div> : null}
        {status}
      </div>
      {actions ? <div className="aw-convhead__actions">{actions}</div> : null}
    </header>
  )
}
