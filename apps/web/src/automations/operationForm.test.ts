import { describe, expect, it } from 'vitest'

import catalog from '../api/generated/catalog.json'
import type { OpenApiDocument } from './api'
import { AUTOMATION_OPERATIONS, operationFields, operationFormValues, operationInput, operationInputSchema } from './operationForm'

const DOCUMENT: OpenApiDocument = {
  info: { title: 'test', version: '1' },
  paths: {
    '/api/v2/w/{workspace}/ops/thing_make': {
      post: {
        operationId: 'thing_make',
        requestBody: { content: { 'application/json': { schema: { $ref: '#/components/schemas/MakeInput' } } } },
      },
    },
  },
  components: {
    schemas: {
      MakeInput: {
        type: 'object',
        required: ['projectId', 'title', 'size'],
        properties: {
          projectId: { type: 'string' },
          clientRequestId: { type: ['string', 'null'] },
          title: { type: 'string', description: 'What it’s called.' },
          description: { type: ['string', 'null'] },
          size: { anyOf: [{ $ref: '#/components/schemas/Size' }, { type: 'null' }] },
          count: { type: ['integer', 'null'] },
          urgent: { type: ['boolean', 'null'] },
          labels: { type: ['array', 'null'], items: { type: 'string' } },
          extra: { type: 'object' },
        },
      },
      Size: { type: 'string', enum: ['small', 'large'] },
    },
  },
}

describe('an operation as a rule action', () => {
  it('asks for its input with the form piece, leaving out what the rule fills in', () => {
    const schema = operationInputSchema(DOCUMENT, 'thing_make')!
    expect(operationFields(schema).map((field) => [field.id, field.label, field.type, field.required ?? false])).toEqual([
      ['title', 'Title', 'text', true],
      ['description', 'Description', 'long_text', false],
      ['size', 'Size', 'single_choice', true],
      ['count', 'Count', 'number', false],
      ['urgent', 'Urgent', 'checkbox', false],
      ['labels', 'Labels', 'long_text', false],
      ['extra', 'Extra', 'long_text', false],
    ])
    expect(operationFields(schema).find((field) => field.id === 'size')?.options).toEqual([
      { value: 'small', label: 'small' },
      { value: 'large', label: 'large' },
    ])
  })

  it('sends typed values with the rule’s space, keeps {{variables}}, and shows them again', () => {
    const schema = operationInputSchema(DOCUMENT, 'thing_make')!
    const input = operationInput(
      schema,
      { title: 'Check {{task.key}}', size: 'small', count: '3', urgent: true, labels: 'a\n\n b ', extra: '{"x": 1}', description: '' },
      'space-1',
    )
    expect(input).toEqual({ projectId: 'space-1', title: 'Check {{task.key}}', size: 'small', count: 3, urgent: true, labels: ['a', 'b'], extra: { x: 1 } })
    expect(operationFormValues(schema, input)).toMatchObject({ title: 'Check {{task.key}}', count: '3', urgent: true, labels: 'a\nb' })
    expect(() => operationInput(schema, { extra: '{nope' }, 'space-1')).toThrow('Extra isn’t valid JSON.')
  })

  it('sends an unticked box when it was touched or the operation needs it, and leaves an untouched one out', () => {
    const schema = operationInputSchema(DOCUMENT, 'thing_make')!
    const base = { title: 'T', size: 'small' }
    expect(operationInput(schema, { ...base, urgent: false }, 'space-1')).toMatchObject({ urgent: false })
    expect(operationInput(schema, base, 'space-1')).not.toHaveProperty('urgent')
    const needed = { ...schema, required: [...(schema.required ?? []), 'urgent'] }
    expect(operationInput(needed, base, 'space-1')).toMatchObject({ urgent: false })
  })

  it('never offers an operation that needs the version of what it changes', () => {
    expect(AUTOMATION_OPERATIONS.some((entry) => entry.name === 'task_update')).toBe(false)
  })

  it('offers only operations the registry marks for automations, never reads or rules themselves', () => {
    expect(AUTOMATION_OPERATIONS.length).toBeGreaterThan(0)
    const marked = new Map((catalog as { name: string; automation: boolean }[]).map((entry) => [entry.name, entry.automation]))
    expect(AUTOMATION_OPERATIONS.every((entry) => marked.get(entry.name) && entry.kind !== 'query' && !entry.name.startsWith('automation_'))).toBe(true)
  })
})
