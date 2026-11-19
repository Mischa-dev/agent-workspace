import { useEffect, useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'

import { describeError } from '../app/mutations'
import { FormPieceFields } from '../pieces/FormPiece'
import { firstMissing, type FormValue, type FormValues } from '../pieces/formFields'
import { Banner, ListSkeleton, SelectField } from '../ui'
import { automationKeys, getOperationDocument } from './api'
import {
  AUTOMATION_OPERATIONS,
  fieldName,
  operationFields,
  operationFormValues,
  operationInput,
  operationInputSchema,
  type CatalogueEntry,
} from './operationForm'

type RunOperation = { kind: 'run_operation'; operation: string; input: Record<string, unknown> }

/** The catalogue by module, for the picker's groups. */
function byModule(entries: readonly CatalogueEntry[]) {
  const groups = new Map<string, CatalogueEntry[]>()
  for (const entry of entries) groups.set(entry.module, [...(groups.get(entry.module) ?? []), entry])
  return [...groups.entries()]
}

/**
 * "Run an operation" (W1.42): any operation an automation may run, picked
 * from the registry's catalogue, its input asked for with the form piece
 * from the operation's own schema. `onProblem` says what stops the rule
 * saving (a required answer missing, a number that isn't one), or null.
 */
export function OperationAction({
  action,
  index,
  onChange,
  onProblem,
  spaceId,
}: {
  action: RunOperation
  index: number
  onChange: (action: RunOperation) => void
  onProblem: (problem: string | null) => void
  spaceId: string
}) {
  const document = useQuery({
    queryKey: automationKeys.operations,
    queryFn: getOperationDocument,
    staleTime: Infinity,
  })
  const schema = useMemo(
    () => (document.data && action.operation ? operationInputSchema(document.data, action.operation) : null),
    [document.data, action.operation],
  )
  const fields = useMemo(() => (schema ? operationFields(schema) : []), [schema])
  // What the person typed, kept as typed; the saved input is built from it.
  const [typed, setTyped] = useState<{ operation: string; values: FormValues } | null>(null)
  const values = typed && typed.operation === action.operation ? typed.values : schema ? operationFormValues(schema, action.input) : {}

  const known = AUTOMATION_OPERATIONS.some((entry) => entry.name === action.operation)
  const problem = !action.operation ? 'Choose an operation to run.' : schema ? firstMissing(fields, values) : null
  useEffect(() => onProblem(problem), [problem, onProblem])

  const change = (id: string, value: FormValue) => {
    if (!schema) return
    const next = { ...values, [id]: value }
    setTyped({ operation: action.operation, values: next })
    try {
      onChange({ ...action, input: operationInput(schema, next, spaceId) })
      onProblem(firstMissing(fields, next))
    } catch (reason) {
      onProblem(describeError(reason).message)
    }
  }

  const id = `rule-action-${index}-operation`
  return (
    <div className="aw-rule-operation" data-part="automation-operation">
      <SelectField
        hint="Anything you could do yourself. It runs as you, in this space."
        id={id}
        label="Operation"
        onChange={(event) => {
          setTyped(null)
          const operation = event.target.value
          const next = document.data ? operationInputSchema(document.data, operation) : null
          onChange({ kind: 'run_operation', operation, input: next ? operationInput(next, {}, spaceId) : {} })
        }}
        value={action.operation}
      >
        <option value="">Choose…</option>
        {!known && action.operation ? <option value={action.operation}>{action.operation}</option> : null}
        {byModule(AUTOMATION_OPERATIONS).map(([module, entries]) => (
          <optgroup key={module} label={fieldName(module)}>
            {entries.map((entry) => (
              <option key={entry.name} value={entry.name}>
                {entry.title}
              </option>
            ))}
          </optgroup>
        ))}
      </SelectField>
      {!action.operation ? null : document.isPending ? (
        <ListSkeleton label="Loading what it asks for" rows={2} />
      ) : document.isError ? (
        <Banner tone="danger">What this operation asks for didn’t load. {describeError(document.error).message}</Banner>
      ) : !schema ? (
        <Banner tone="warning">This operation isn’t offered any more. Choose another.</Banner>
      ) : fields.length === 0 ? (
        <p className="aw-quiet">It needs nothing else.</p>
      ) : (
        <>
          <FormPieceFields fields={fields.filter((field) => field.required)} onChange={change} values={values} />
          {fields.some((field) => !field.required) ? (
            <details className="aw-disclosure" open={fields.some((field) => !field.required && values[field.id] !== undefined && values[field.id] !== '')}>
              <summary>More options</summary>
              <div className="aw-rule-operation">
                <FormPieceFields fields={fields.filter((field) => !field.required)} onChange={change} values={values} />
              </div>
            </details>
          ) : null}
        </>
      )}
    </div>
  )
}
