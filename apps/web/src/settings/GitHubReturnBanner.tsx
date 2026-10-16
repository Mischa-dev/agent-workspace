import { useEffect, useState } from 'react'
import { useSearchParams } from 'react-router-dom'

import { githubReturnMessage } from '../github/connect'
import { Banner } from '../ui'

/**
 * What GitHub's return (`?github=connected` or `?github=error&reason=`) means,
 * read once and then taken out of the address. Most returns land on a Project's
 * Repositories page; one whose connect link expired or belonged to another
 * browser lands on /settings, which has no Project in it.
 */
export function GitHubReturnBanner({ className }: { className?: string }) {
  const [parameters, setParameters] = useSearchParams()
  const [returned] = useState(() => githubReturnMessage(parameters.get('github'), parameters.get('reason')))
  useEffect(() => {
    if (!parameters.has('github')) return
    const next = new URLSearchParams(parameters)
    next.delete('github')
    next.delete('reason')
    setParameters(next, { replace: true })
  }, [parameters, setParameters])
  if (!returned) return null
  return (
    <Banner className={className} tone={returned.tone}>
      {returned.text}
    </Banner>
  )
}
