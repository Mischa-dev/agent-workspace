import { render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { describe, expect, it } from 'vitest'

import { Markdown } from './Markdown'
import { parseMarkdown, safeHref } from './parse'

function renderMarkdown(source: string, taskHref?: (key: string) => string | null) {
  return render(
    <MemoryRouter>
      <Markdown source={source} taskHref={taskHref} />
    </MemoryRouter>,
  )
}

describe('Markdown', () => {
  it('shows an embed as a link, or as whatever draws it', () => {
    const source = 'Feedback:\n\n[Playtest feedback](/projects/p/collections/c "embed")'
    const { container, rerender } = renderMarkdown(source)
    expect(screen.getByRole('link', { name: 'Playtest feedback' })).toHaveAttribute('href', '/projects/p/collections/c')
    rerender(
      <MemoryRouter>
        <Markdown renderEmbed={(embed) => <figure data-href={embed.href}>{embed.label}</figure>} source={source} />
      </MemoryRouter>,
    )
    expect(container.querySelector('figure')).toHaveAttribute('data-href', '/projects/p/collections/c')
    expect(screen.queryByRole('link')).toBeNull()
  })

  it('shows raw HTML as text and never creates elements from it', () => {
    const { container } = renderMarkdown('<script>alert(1)</script>\n\n<img src=x onerror=alert(1)> **bold**')
    expect(container.querySelector('script')).toBeNull()
    expect(container.querySelector('img')).toBeNull()
    expect(container).toHaveTextContent('<script>alert(1)</script>')
    expect(container).toHaveTextContent('<img src=x onerror=alert(1)>')
    expect(container.querySelector('strong')).toHaveTextContent('bold')
  })

  it('only links safe destinations', () => {
    const { container } = renderMarkdown(
      [
        '[bad](javascript:alert(1)) [data](data:text/html,hi) [vb](vbscript:x) [proto](//evil.example)',
        '[ok](https://example.com/a) [inside](/projects/1) <https://example.org> https://example.net/x.',
      ].join('\n'),
    )
    const links = [...container.querySelectorAll('a')].map((a) => a.getAttribute('href'))
    expect(links).toEqual(['https://example.com/a', '/projects/1', 'https://example.org', 'https://example.net/x'])
    expect(screen.getByText(/bad/)).not.toHaveAttribute('href')
    const external = screen.getByRole('link', { name: 'ok' })
    expect(external).toHaveAttribute('target', '_blank')
    expect(external).toHaveAttribute('rel', 'noreferrer noopener')
    expect(screen.getByRole('link', { name: 'inside' })).not.toHaveAttribute('target')
    expect(safeHref(' JaVaScRiPt:alert(1)')).toBeNull()
    expect(safeHref('java\tscript:alert(1)')).toBeNull()
    expect(safeHref('/\\evil.example')).toBeNull()
  })

  it('renders a task description with a checklist and code', async () => {
    const { container } = renderMarkdown(
      [
        '## Plan',
        '',
        '- [x] Write the parser',
        '- [ ] Ship it, see AWD-12',
        '  - nested note',
        '',
        '```ts',
        'const key = "AWD-13" // not a link',
        '```',
        '',
        'Inline `AWD-14` stays code; [AWD-15 docs](/docs) stays one link.',
      ].join('\n'),
    )
    expect(screen.getByRole('heading', { level: 2, name: 'Plan' })).toBeInTheDocument()
    const boxes = screen.getAllByRole('checkbox')
    expect(boxes).toHaveLength(2)
    expect(boxes[0]).toBeChecked()
    expect(boxes[1]).not.toBeChecked()
    expect(boxes[0]).toBeDisabled()
    expect(container.querySelector('li li')).toHaveTextContent('nested note')

    const code = container.querySelector('pre > code')
    expect(code).toHaveClass('language-ts')
    expect(code).toHaveTextContent('const key = "AWD-13" // not a link')

    expect(screen.getByRole('link', { name: 'AWD-12' })).toHaveAttribute('href', '/AWD-12')
    expect(screen.queryByRole('link', { name: 'AWD-13' })).toBeNull()
    expect(screen.queryByRole('link', { name: 'AWD-14' })).toBeNull()
    expect(screen.getByRole('link', { name: 'AWD-15 docs' })).toHaveAttribute('href', '/docs')
    expect(container.querySelectorAll('a')).toHaveLength(2)

    // The highlighter arrives later and colours the same text.
    await waitFor(() => expect(code?.querySelector('.aw-md-tok--keyword')).toHaveTextContent('const'))
    expect(code).toHaveTextContent('const key = "AWD-13" // not a link')
  })

  it('uses the task link the caller gives, or none', () => {
    renderMarkdown('AWD-1 and OPS-2 but not UTF-8', (key) => (key.startsWith('AWD') ? `/projects/p/work/${key}` : null))
    expect(screen.getByRole('link', { name: 'AWD-1' })).toHaveAttribute('href', '/projects/p/work/AWD-1')
    expect(screen.queryByRole('link', { name: 'OPS-2' })).toBeNull()
    expect(screen.queryByRole('link', { name: 'UTF-8' })).toBeNull()
  })

  it('renders tables, quotes and emphasis', () => {
    const { container } = renderMarkdown('> a _quote_ ~~gone~~\n\n| Name | Count |\n| :--- | ---: |\n| a | 1 |\n\nsnake_case_name stays')
    expect(container.querySelector('blockquote em')).toHaveTextContent('quote')
    expect(container.querySelector('blockquote del')).toHaveTextContent('gone')
    expect(screen.getByRole('columnheader', { name: 'Count' })).toHaveStyle({ textAlign: 'right' })
    expect(screen.getByRole('cell', { name: '1' })).toBeInTheDocument()
    expect(container).toHaveTextContent('snake_case_name stays')
  })

  it('stays fast and shallow on hostile input', () => {
    const started = performance.now()
    parseMarkdown('*'.repeat(50_000) + '_a'.repeat(20_000) + '['.repeat(20_000) + '`'.repeat(5_000))
    parseMarkdown('> '.repeat(5_000) + 'deep')
    parseMarkdown(Array.from({ length: 2_000 }, (_, depth) => `${'  '.repeat(depth)}- item`).join('\n'))
    expect(performance.now() - started).toBeLessThan(3_000)
  })
})
