import { ButtonLink } from '../ui'

/** A section's way to the page that holds everything it shows, such as "Inbox ›". */
export function MoreLink({ children, to }: { children: string; to: string }) {
  return (
    <ButtonLink iconEnd="chevron-right" size="sm" to={to} variant="ghost">
      {children}
    </ButtonLink>
  )
}
