# M8 accessibility review

This note records the code-level M8-005 audit and the evidence still required
from the exact private-beta commit. It is not a WCAG conformance claim or a
VPAT.

## Audit result

The initial technical audit scored 16/20. The UI already had semantic
landmarks, a skip link, visible focus, keyboard-equivalent board controls,
inert hostile output, responsive navigation drawers, forced-color treatment,
and a global reduced-motion rule. Four bounded findings were corrected:

| Severity | Finding | Correction |
| --- | --- | --- |
| P1 | The New Work Item modal did not contain focus, close on Escape, make the application inert, or restore focus. | Render the dialog in a portal, inert the application root during its lifetime, trap Tab and Shift+Tab, support Escape, and return focus to the invoking control. |
| P1 | White 12.8 px text on the active blue Project surface measured 3.75:1. | Use the existing dark accent-ink token; the measured pair is 6.81:1. |
| P2 | Several phone-width controls retained 30-40 px touch targets. | Apply a 44 px minimum target to mobile navigation, form, composer, link, picker, and Work Item action controls. |
| P2 | Two bordered floating surfaces also used broad decorative shadows. | Remove the modal shadow and bound the inline-picker shadow to an 8 px blur. |

The post-correction code audit is 18/20: accessibility, theming, responsive
behavior, and the product visual vocabulary have no known automated blocker;
the remaining two points are withheld until the manual exact-browser and
assistive-technology pass below is recorded.

## Automated evidence

The route fixtures use `axe-core` 4.12.1 and require an empty violation list
for:

- signed-out sign-in and authenticated four-region shell
- Project search and narrow navigation drawers
- invitation join with hostile Project text
- Work Item list, board keyboard alternatives, and the modal focus lifecycle
- hosted Thread timeline with hostile provider output and an approval
- Team membership and role controls with hostile identity text

The jsdom fixtures disable only axe's `color-contrast` rule because jsdom has
no rendered pixels or Canvas implementation. No other rule is suppressed. The
committed OKLCH pairs were independently converted to linear sRGB for the
code-level audit. Representative ratios are 16.04:1 for primary text on the
application background, 6.16:1 for muted text on the background, 5.51:1 for
muted text on the raised surface, and 6.81:1 for accent ink on the active
accent. The exact-browser pass must re-run rendered contrast and cannot rely on
these calculations alone.

The web suite also asserts focus restoration for Project search and responsive
navigation, modal containment and Escape behavior, disabled-control reasons,
keyboard-equivalent board movement, inert hostile output, and coalesced
streaming announcements. The production build retains both the
`forced-colors: active` focus/state rules and the global
`prefers-reduced-motion: reduce` override.

Run the deterministic local gate with the pinned runtime:

```bash
fnm exec --using 24.18.1 corepack pnpm lint
fnm exec --using 24.18.1 corepack pnpm typecheck
fnm exec --using 24.18.1 corepack pnpm test
fnm exec --using 24.18.1 corepack pnpm build
```

## Exact-commit manual gate

Before M8 acceptance, append the exact commit, macOS and Chrome versions, and
results for all of these checks against the SSH-forwarded Red Hat application:

1. Traverse sign-in, Project switching, Channel/Thread, composer mentions,
   Work Items, hosted approvals, Team settings, profile setup, and archive
   states with keyboard only. Confirm visible focus, logical order, no trap,
   Escape behavior, and focus restoration.
2. Repeat the two-human critical path with macOS VoiceOver. Record each route,
   announced control/state, correction, and any bounded residual limitation.
3. Verify narrow navigation at 390 CSS px, browser zoom at 200%, forced colors,
   and reduced motion. No control, error, conflict, priority, check, or run state
   may depend on color alone.
4. Run browser axe on the rendered critical routes with contrast enabled and
   accept zero serious or critical violations and no unrecorded suppression.
5. Confirm streamed deltas remain coalesced, hostile output remains text, and
   high-frequency status does not create unbounded live-region announcements.

Any P0/P1 result blocks the private-beta gate. P2 results require a correction
or an explicit bounded residual-risk entry before M8 can be accepted.
