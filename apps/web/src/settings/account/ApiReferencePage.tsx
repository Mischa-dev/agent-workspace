import { useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'

import { apiUrl } from '../../api/client'
import { automationKeys, getOpenApi, type OpenApiDocument, type OpenApiOperation, type OpenApiSchema } from '../../automations/api'
import '../../automations/automations.css'
import { Button, EmptyState, ErrorState, Icon, ListSkeleton, PageHeader, Section, StatusChip, TextField, showToast } from '../../ui'
import { accountSettingsHref } from '../paths'

type Operation = { path: string; operation: OpenApiOperation }

function resolve(document: OpenApiDocument, schema: OpenApiSchema | undefined): OpenApiSchema | undefined {
  const reference = schema?.$ref
  if (!reference) return schema
  return document.components?.schemas?.[reference.replace('#/components/schemas/', '')]
}

/** "string", "integer or null", "one of: a, b": short enough for a table cell. */
function describeType(document: OpenApiDocument, schema: OpenApiSchema | undefined, depth = 0): string {
  const resolved = resolve(document, schema)
  if (!resolved || depth > 3) return 'any'
  if (resolved.enum) return `one of: ${resolved.enum.map(String).join(', ')}`
  if (resolved.const !== undefined) return JSON.stringify(resolved.const)
  const variants = resolved.oneOf ?? resolved.anyOf
  if (variants) {
    const kinds = variants.map((variant) => {
      const inner = resolve(document, variant)
      const tag = inner?.properties?.kind?.const ?? inner?.properties?.kind?.enum?.[0]
      return tag !== undefined ? String(tag) : describeType(document, variant, depth + 1)
    })
    return kinds.join(' or ')
  }
  const type = Array.isArray(resolved.type) ? resolved.type.join(' or ') : resolved.type
  if (type === 'array' || (Array.isArray(resolved.type) && resolved.type.includes('array'))) {
    return `list of ${describeType(document, resolved.items, depth + 1)}`
  }
  if (!type && resolved.properties) return 'object'
  return type ?? 'any'
}

function exampleValue(document: OpenApiDocument, schema: OpenApiSchema | undefined): unknown {
  const resolved = resolve(document, schema)
  if (!resolved) return null
  if (resolved.enum?.length) return resolved.enum[0]
  const type = Array.isArray(resolved.type) ? resolved.type.find((kind) => kind !== 'null') : resolved.type
  switch (type) {
    case 'string':
      return '…'
    case 'integer':
    case 'number':
      return 1
    case 'boolean':
      return true
    case 'array':
      return []
    case 'object':
      return {}
    default:
      return null
  }
}

function curlFor(document: OpenApiDocument, base: string, entry: Operation) {
  const schema = resolve(document, entry.operation.requestBody?.content?.['application/json']?.schema)
  const required = schema?.required ?? []
  const body = Object.fromEntries(required.map((name) => [name, exampleValue(document, schema?.properties?.[name])]))
  return [
    `curl -X POST ${base}${entry.path} \\`,
    '  -H "Authorization: Bearer $AGENT_WORKSPACE_TOKEN" \\',
    '  -H "Content-Type: application/json" \\',
    `  -d '${JSON.stringify(body)}'`,
  ].join('\n')
}

/** Account › API reference: every operation of the public API, from its OpenAPI document. */
export default function ApiReferencePage() {
  const openapi = useQuery({
    queryKey: automationKeys.openapi,
    queryFn: getOpenApi,
    staleTime: 5 * 60_000,
  })
  const [filter, setFilter] = useState('')
  const base = new URL(apiUrl(openapi.data?.servers?.[0]?.url ?? '/api/public/v1'), window.location.href).href

  const groups = useMemo(() => {
    if (!openapi.data) return []
    const needle = filter.trim().toLowerCase()
    const byTag = new Map<string, Operation[]>()
    for (const [path, item] of Object.entries(openapi.data.paths)) {
      const operation = item.post
      if (!operation) continue
      const haystack = `${operation.operationId} ${operation.summary ?? ''} ${operation.description ?? ''}`.toLowerCase()
      if (needle && !haystack.includes(needle)) continue
      const tag = operation.tags?.[0] ?? 'Other'
      byTag.set(tag, [...(byTag.get(tag) ?? []), { path, operation }])
    }
    return [...byTag.entries()].sort(([a], [b]) => a.localeCompare(b))
  }, [openapi.data, filter])

  return (
    <>
      <PageHeader
        actions={
          <a className="aw-btn aw-btn--secondary aw-btn--md" download href={apiUrl('/api/public/v1/openapi.json')}>
            <Icon name="arrow-down" size={16} />
            OpenAPI document
          </a>
        }
        icon="circle-help"
        title="API reference"
      >
        Everything the app and agents can do, as REST. Each operation is a POST of JSON to{' '}
        <code className="aw-inline-code">{base}/&lt;operation&gt;</code> with a personal token from{' '}
        <Link to={accountSettingsHref('api-tokens')}>API tokens</Link>; it acts as you, in the token’s project.
      </PageHeader>
      {openapi.isPending ? (
        <ListSkeleton label="Loading the API reference" rows={4} />
      ) : openapi.isError ? (
        <ErrorState error={openapi.error} onRetry={() => void openapi.refetch()} title="The API reference didn’t load" />
      ) : (
        <>
          <Section id="api-basics-heading" title="How it works">
            <ul className="aw-api-basics">
              <li>
                Send <code className="aw-inline-code">Authorization: Bearer &lt;token&gt;</code>. Read-only tokens can call the operations
                marked Read.
              </li>
              <li>
                Every reply is JSON with an <code className="aw-inline-code">outcome</code>: <code className="aw-inline-code">ok</code> or{' '}
                <code className="aw-inline-code">applied</code> when it worked; otherwise a 4xx status with a message saying what to change.
              </li>
              <li>
                Projects and tasks can be named by ID, link or key (such as <code className="aw-inline-code">WORK-12</code>). Pass a{' '}
                <code className="aw-inline-code">clientRequestId</code> where offered so a retry never does it twice.
              </li>
            </ul>
          </Section>
          <TextField
            aria-label="Find an operation"
            id="api-filter"
            onChange={(event) => setFilter(event.target.value)}
            placeholder="Find an operation, such as task or webhook"
            type="search"
            value={filter}
          />
          {groups.length ? (
            groups.map(([tag, operations]) => (
              <Section id={`api-${tag.toLowerCase().replace(/\W+/g, '-')}`} key={tag} title={tag}>
                <div className="aw-api-ops">
                  {operations.map((entry) => (
                    <OperationCard base={base} document={openapi.data} entry={entry} key={entry.path} />
                  ))}
                </div>
              </Section>
            ))
          ) : (
            <EmptyState action={<Button onClick={() => setFilter('')}>Clear the search</Button>} icon="search" title="No operation matches">
              Try another word.
            </EmptyState>
          )}
        </>
      )}
    </>
  )
}

function OperationCard({ base, document, entry }: { base: string; document: OpenApiDocument; entry: Operation }) {
  const schema = resolve(document, entry.operation.requestBody?.content?.['application/json']?.schema)
  const required = new Set(schema?.required ?? [])
  const fields = Object.entries(schema?.properties ?? {})
  const curl = curlFor(document, base, entry)
  return (
    <details className="aw-api-op">
      <summary>
        <code className="aw-api-op__name">{entry.operation.operationId}</code>
        <span className="aw-api-op__summary">{entry.operation.summary}</span>
        {entry.operation['x-read-only'] ? <StatusChip tone="muted">Read</StatusChip> : <StatusChip tone="agent">Write</StatusChip>}
      </summary>
      <div className="aw-api-op__body">
        {entry.operation.description && <p>{entry.operation.description}</p>}
        {fields.length ? (
          <div className="aw-api-params__scroll">
            <table className="aw-api-params">
              <thead>
                <tr>
                  <th scope="col">Field</th>
                  <th scope="col">Type</th>
                  <th scope="col">About</th>
                </tr>
              </thead>
              <tbody>
                {fields.map(([name, field]) => (
                  <tr key={name}>
                    <td>
                      <code className="aw-inline-code">{name}</code>
                      {required.has(name) && <span className="aw-api-params__required">required</span>}
                    </td>
                    <td>{describeType(document, field)}</td>
                    <td>{resolve(document, field)?.description ?? field.description ?? ''}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <p className="aw-quiet">No fields: send {'{}'}.</p>
        )}
        <div className="aw-api-op__curl">
          <pre>
            <code>{curl}</code>
          </pre>
          <Button
            icon="copy"
            onClick={() => {
              const failed = () =>
                showToast({ tone: 'danger', title: 'Couldn’t copy', description: 'Select the command and copy it yourself.' })
              if (!navigator.clipboard) failed()
              else navigator.clipboard.writeText(curl).then(() => showToast({ title: 'Command copied' }), failed)
            }}
            size="sm"
            variant="ghost"
          >
            Copy
          </Button>
        </div>
      </div>
    </details>
  )
}
