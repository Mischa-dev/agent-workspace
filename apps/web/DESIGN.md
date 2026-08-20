# Design System

## Theme

A restrained operational interface for technical work in mixed-light
environments. Dark is the default product expression; Light and System are
complete, supported modes. Theme and density are presentation-only preferences
stored locally and cannot affect identity, authority, capabilities, or durable
workspace state.

## Color

Primitive OKLCH values feed semantic surface, text, border, action, status, and
identity roles. Deep neutral surfaces establish hierarchy; blue marks selection
and primary action, amber marks attention, red marks failure, green marks
success, and gray marks settled history. Every state includes text or an icon in
addition to color. Custom themes may override only the validated accent and
identity roles documented in `docs/frontend/theme-and-tokens.md`.

## Typography

Use the existing system sans-serif stack for all interface text. Keep the scale
compact and fixed: 12–14px metadata and labels, 14–16px body and controls, and
16–20px page headings. Use the monospace stack only for commits, branches,
paths, and event payloads.

## Layout

The desktop shell has four regions: 64 px Project rail, 248 px Project sidebar,
flexible main workspace, and 288 px context region. At tablet widths context is
an explicit modal sheet. At phone widths the main resource is the only
persistent region and the rail, Project sidebar, and context become independent
drawers. The main Thread surface owns scrolling while the composer stays
available.

## Components

- Project controls and Channels use one compact selection vocabulary.
- The narrow Project rail distinguishes pinned, recent, active, and
  attention-bearing Projects without becoming a second full sidebar.
- Sign-in, Project creation, and archive confirmation use ordinary labelled
  forms with focused error summaries and explicit outcomes.
- Timeline entries are grouped by meaning, not wrapped in identical cards.
- Provider events use quiet bordered rows with a visible event type and summary.
- Pending approvals use a contained alert surface with explicit allow and deny
  actions.
- Run status uses icon, label, and semantic color.
- Buttons, text areas, and focus rings share the same radii and state behavior.

## Motion

Use 150–200ms state transitions for hover, focus, and newly arriving timeline
content. Avoid page choreography. Disable nonessential transitions under
`prefers-reduced-motion`.

## Density and interaction

Compact, Comfortable, and Spacious adjust row and control rhythm without
shrinking readable type or mobile targets below 44 px. Focus is always visible.
Dialogs and drawers make inactive regions inert, contain keyboard focus, close
on Escape or backdrop, and restore the originating control.

## Product boundaries

Keep one application tree, router, server-state model, and responsive shell.
Navigation exposes accepted Project, Channel, Thread, Work Item, and settings
behavior only. A notification center or command palette remains deferred until
its durable server contract exists. No final public logo or decorative identity
system is implied by this operational UI.
