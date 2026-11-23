import { Link } from 'react-router-dom'

import { isApiError } from '../../api/errors'
import { Button, Icon } from '../../ui'
import './thingEmbed.css'

// The pieces embeds of things share: a link, and a problem line.

export function EmbedLink({ href, label }: { href: string; label: string }) {
  return (
    <Link className="aw-embed__link" to={href}>
      <Icon name="link" size={16} />
      <span>{label || href}</span>
    </Link>
  )
}

export function EmbedProblem({ error, href, label, onRetry }: { error: unknown; href: string; label: string; onRetry: () => void }) {
  const hidden = isApiError(error) && (error.status === 404 || error.status === 403)
  return (
    <p className="aw-membed__problem">
      <Icon name="link" size={16} />
      <span>
        {hidden ? (
          <>
            <Link to={href}>{label || 'This'}</Link> isn’t here, or you can’t see it.
          </>
        ) : (
          `${label || 'This'} didn’t load.`
        )}
      </span>
      {hidden ? null : (
        <Button onClick={onRetry} size="sm">
          Try again
        </Button>
      )}
    </p>
  )
}
