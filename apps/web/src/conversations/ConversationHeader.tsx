import type { ReactNode } from 'react'
import { Link } from 'react-router-dom'

import { MenuButton } from '../shell/MenuButton'
import { Icon, Menu, type IconName, type MenuEntry } from '../ui'

/**
 * The bar above a conversation: what you're in, a short subtitle, and tools on the right.
 * The title is the page's h1; with a menu it also opens the conversation's actions.
 * On phones it also leads with the navigation menu, or with `back` in a thread.
 */
export function ConversationHeader({
  actions,
  back,
  icon,
  lead,
  menu,
  status,
  subtitle,
  tag,
  title,
}: {
  actions?: ReactNode
  /** Where a phone's back arrow goes, such as a thread's channel. Replaces the menu button there. */
  back?: { label: string; to: string }
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
      {back ? (
        <Link aria-label={back.label} className="aw-convhead__back" to={back.to}>
          <Icon name="arrow-left" size={20} />
        </Link>
      ) : (
        <MenuButton />
      )}
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
