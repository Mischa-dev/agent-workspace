import { SLOT_GROUPS } from '../theme/slots'
import { Section } from '../ui'

// The scales in styles/tokens.css, drawn with the tokens themselves so each
// specimen shows what the current theme and text size resolve to.

const TYPE = [
  ['display', 'Document titles: a task, a page'],
  ['headline', 'Page titles (PageHeader)'],
  ['title', 'Dialog titles and page sections'],
  ['heading', 'Card titles, small sections'],
  ['body', 'Messages, descriptions, reading text'],
  ['ui', 'Controls, rows, menus'],
  ['ui-strong', 'Buttons, row titles, names'],
  ['label', 'Field labels, column headers'],
  ['small', 'Secondary lines, hints, meta'],
  ['caption', 'Timestamps, counts'],
  ['caption-strong', 'Group labels, small emphasis'],
  ['tag', 'Chips and tags'],
  ['code', 'Code blocks'],
  ['code-sm', 'Keys, branches, ids'],
] as const

const SPACE = [
  ['half', 2],
  ['1', 4],
  ['1-5', 6],
  ['2', 8],
  ['2-5', 10],
  ['3', 12],
  ['4', 16],
  ['5', 20],
  ['6', 24],
  ['8', 32],
  ['10', 40],
  ['12', 48],
] as const

const RADII = [
  ['xs', 'Tags, kbd, inline code'],
  ['sm', 'Buttons, fields, menu items'],
  ['md', 'Cards, lists, menus, popovers'],
  ['lg', 'Dialogs, page marks'],
  ['full', 'Chips, avatars, badges'],
] as const

const SHADOWS = [
  ['card', 'Cards and bordered lists (flat in dark themes)'],
  ['popover', 'Menus, pickers, tooltips, toasts'],
  ['dialog', 'Dialogs and drawers'],
  ['focus', 'Focus inside a clipped box; elsewhere an outline'],
] as const

const MOTION = [
  ['fast', '120ms', 'Hovers, presses'],
  ['standard', '180ms', 'Menus, tooltips, toasts'],
  ['slow', '260ms', 'Dialogs, drawers'],
  ['fade', '1200ms', 'A highlight fading out'],
] as const

export function TokenSpecimens() {
  return (
    <>
      <Section description="Every color is a theme slot people can change on the Appearance page. Components use these names, never a color." title="Color">
        <div className="aw-ds__colors">
          {SLOT_GROUPS.map((group) => (
            <div className="aw-ds__colorgroup" key={group.label}>
              <h3 className="aw-ds__label">{group.label}</h3>
              <ul className="aw-ds__swatches">
                {group.slots.map(([slot, label]) => (
                  <li key={slot}>
                    <span aria-hidden="true" className="aw-ds__swatch" style={{ background: `var(--${slot})` }} />
                    <span className="aw-ds__name">{label}</span>
                    <code>--{slot}</code>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      </Section>

      <Section description="Figtree for everything, JetBrains Mono for code. Each style is one font shorthand that follows the text size setting." title="Type">
        <ul className="aw-ds__type">
          {TYPE.map(([name, use]) => (
            <li key={name}>
              <span className="aw-ds__sample" style={{ font: `var(--t-${name})` }}>
                {name.startsWith('code') ? 'feature/export-csv' : 'Ship the export'}
              </span>
              <code>--t-{name}</code>
              <span className="aw-ds__use">{use}</span>
            </li>
          ))}
        </ul>
      </Section>

      <Section description="A 4px grid. Half steps (2, 6, 10) are for dense controls: chips, menu items, rows." title="Spacing">
        <ul className="aw-ds__scale">
          {SPACE.map(([name, px]) => (
            <li key={name}>
              <span aria-hidden="true" className="aw-ds__bar" style={{ width: `var(--space-${name})` }} />
              <code>--space-{name}</code>
              <span className="aw-ds__use">{px}px</span>
            </li>
          ))}
        </ul>
      </Section>

      <Section title="Corners, depth and motion">
        <div className="aw-ds__tiles">
          {RADII.map(([name, use]) => (
            <figure className="aw-ds__tile" key={name}>
              <span aria-hidden="true" className="aw-ds__shape" style={{ borderRadius: `var(--radius-${name})` }} />
              <figcaption>
                <code>--radius-{name}</code>
                <span className="aw-ds__use">{use}</span>
              </figcaption>
            </figure>
          ))}
          {SHADOWS.map(([name, use]) => (
            <figure className="aw-ds__tile" key={name}>
              <span aria-hidden="true" className="aw-ds__shape aw-ds__shape--raised" style={{ boxShadow: `var(--shadow-${name})` }} />
              <figcaption>
                <code>--shadow-{name}</code>
                <span className="aw-ds__use">{use}</span>
              </figcaption>
            </figure>
          ))}
        </div>
        <ul className="aw-ds__scale">
          {MOTION.map(([name, ms, use]) => (
            <li key={name}>
              <code>--duration-{name}</code>
              <span className="aw-ds__use">
                {ms} · {use}
              </span>
            </li>
          ))}
        </ul>
      </Section>
    </>
  )
}
