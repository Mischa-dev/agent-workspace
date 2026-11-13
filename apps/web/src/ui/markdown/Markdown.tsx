import { Fragment, useMemo, type ReactNode } from 'react'
import { Link, useInRouterContext } from 'react-router-dom'

import { CodeBlock } from './CodeBlock'
import { parseMarkdown, safeHref, type Block, type Embed, type Inline } from './parse'

export type MarkdownProps = {
  /** The Markdown source. Raw HTML in it is shown as text. */
  source: string
  /**
   * Where a task key such as AWD-12 links to. Return null to leave the key as
   * plain text. Defaults to `/AWD-12`.
   */
  taskHref?: (key: string) => string | null
  className?: string
  /** Shown when the source is empty or only whitespace. */
  empty?: ReactNode
  /**
   * Renders plain text runs, for example to turn placeholders back into
   * mention chips. Text in code is passed through `codeText` instead.
   */
  renderText?: (value: string) => ReactNode
  codeText?: (value: string) => string
  /** Draws an embed, such as a collection's view in a page; without it, an embed is a link. */
  renderEmbed?: (embed: Embed) => ReactNode
  /**
   * Draws a link in place of the plain one, such as a chip for a task or a
   * page (AWD-58); `bare` when its text is the address itself. Return null
   * to keep the plain link.
   */
  renderLink?: (href: string, children: ReactNode, bare: boolean) => ReactNode
}

const defaultTaskHref = (key: string) => `/${key}`

type RenderContext = {
  taskHref: (key: string) => string | null
  router: boolean
  renderText?: (value: string) => ReactNode
  codeText: (value: string) => string
  renderEmbed?: (embed: Embed) => ReactNode
  renderLink?: (href: string, children: ReactNode, bare: boolean) => ReactNode
}

const asIs = (value: string) => value

/** Renders Markdown as React elements; never as HTML. */
export function Markdown({ className, codeText = asIs, empty = null, renderEmbed, renderLink: drawLink, renderText, source, taskHref = defaultTaskHref }: MarkdownProps) {
  const router = useInRouterContext()
  const blocks = useMemo(() => parseMarkdown(source), [source])
  const classes = ['aw-md', className].filter(Boolean).join(' ')
  if (!source.trim()) return empty == null ? null : <div className={classes}>{empty}</div>
  return <div className={classes}>{renderBlocks(blocks, { taskHref, router, renderText, codeText, renderEmbed, renderLink: drawLink })}</div>
}

function renderBlocks(blocks: Block[], context: RenderContext): ReactNode[] {
  return blocks.map((block, index) => renderBlock(block, index, context))
}

function renderBlock(block: Block, key: number, context: RenderContext): ReactNode {
  switch (block.type) {
    case 'heading': {
      const Tag = `h${block.level}` as const
      return <Tag key={key}>{renderInlines(block.children, context)}</Tag>
    }
    case 'paragraph':
      return <p key={key}>{renderInlines(block.children, context)}</p>
    case 'code':
      return <CodeBlock key={key} language={block.lang} value={context.codeText(block.value)} />
    case 'quote':
      return <blockquote key={key}>{renderBlocks(block.children, context)}</blockquote>
    case 'hr':
      return <hr key={key} />
    case 'embed':
      return context.renderEmbed ? (
        <Fragment key={key}>{context.renderEmbed(block)}</Fragment>
      ) : (
        <p key={key}>{renderLink(block.href, false, block.label || block.href, key, context)}</p>
      )
    case 'overflow':
      return (
        <p className="aw-md__overflow" key={key}>
          {context.codeText(block.value)}
        </p>
      )
    case 'list': {
      const hasTasks = block.items.some((item) => item.checked !== null)
      const items = block.items.map((item, index) => (
        <li className={item.checked !== null ? 'aw-md__task' : undefined} key={index}>
          {item.checked !== null ? (
            <input aria-label={item.checked ? 'Done' : 'Not done'} checked={item.checked} disabled readOnly type="checkbox" />
          ) : null}
          {renderItem(item.children, context)}
        </li>
      ))
      const className = hasTasks ? 'aw-md__tasks' : undefined
      return block.ordered ? (
        <ol className={className} key={key} start={block.start === 1 ? undefined : block.start}>
          {items}
        </ol>
      ) : (
        <ul className={className} key={key}>
          {items}
        </ul>
      )
    }
    case 'table':
      return (
        <div className="aw-md__table" key={key}>
          <table>
            <thead>
              <tr>
                {block.head.map((cell, column) => (
                  <th key={column} style={block.align[column] ? { textAlign: block.align[column] } : undefined}>
                    {renderInlines(cell, context)}
                  </th>
                ))}
              </tr>
            </thead>
            {block.rows.length > 0 ? (
              <tbody>
                {block.rows.map((row, rowIndex) => (
                  <tr key={rowIndex}>
                    {row.map((cell, column) => (
                      <td key={column} style={block.align[column] ? { textAlign: block.align[column] } : undefined}>
                        {renderInlines(cell, context)}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            ) : null}
          </table>
        </div>
      )
  }
}

/** A list item made of one paragraph renders without the paragraph wrapper, so lists stay tight. */
function renderItem(children: Block[], context: RenderContext): ReactNode {
  const [first, ...rest] = children
  if (first?.type === 'paragraph') {
    return (
      <>
        <span className="aw-md__item-text">{renderInlines(first.children, context)}</span>
        {renderBlocks(rest, context)}
      </>
    )
  }
  return renderBlocks(children, context)
}

function renderInlines(inlines: Inline[], context: RenderContext): ReactNode[] {
  return inlines.map((inline, index) => renderInline(inline, index, context))
}

function renderInline(inline: Inline, key: number, context: RenderContext): ReactNode {
  switch (inline.type) {
    case 'text':
      return context.renderText ? <Fragment key={key}>{context.renderText(inline.value)}</Fragment> : inline.value
    case 'strong':
      return <strong key={key}>{renderInlines(inline.children, context)}</strong>
    case 'em':
      return <em key={key}>{renderInlines(inline.children, context)}</em>
    case 'del':
      return <del key={key}>{renderInlines(inline.children, context)}</del>
    case 'code':
      return (
        <code className="aw-md__code" key={key}>
          {context.codeText(inline.value)}
        </code>
      )
    case 'br':
      return <br key={key} />
    case 'link': {
      const children = renderInlines(inline.children, context)
      const only = inline.children.length === 1 ? inline.children[0] : null
      const drawn = context.renderLink?.(inline.href, children, only?.type === 'text' && only.value === inline.href)
      if (drawn != null) return <Fragment key={key}>{drawn}</Fragment>
      return renderLink(inline.href, inline.external, children, key, context)
    }
    case 'taskKey': {
      const target = context.taskHref(inline.key)
      const safe = target ? safeHref(target) : null
      if (!safe) return inline.key
      return renderLink(safe.href, safe.external, inline.key, key, context, 'aw-md__task-key')
    }
  }
}

function renderLink(href: string, external: boolean, children: ReactNode, key: number, context: RenderContext, className?: string) {
  if (external) {
    return (
      <a className={className} href={href} key={key} rel="noreferrer noopener" target={href.startsWith('mailto:') ? undefined : '_blank'}>
        {children}
      </a>
    )
  }
  return context.router ? (
    <Link className={className} key={key} to={href}>
      {children}
    </Link>
  ) : (
    <a className={className} href={href} key={key}>
      {children}
    </a>
  )
}
