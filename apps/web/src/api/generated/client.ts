// Generated from the operation registry by `ops::codegen` in the server.
// Do not edit: change the operation, then run
// OPS_BLESS=1 cargo test -p agent-workspace-server --lib generated_files_are_current

import { CURRENT_WORKSPACE, currentWorkspaceSlug } from '../../app/href'
import { requestJson } from '../client'
import { ApiError } from '../errors'
import type { Envelope, OpName, Operations } from './operations'

export type CallOptions = {
  /** The workspace to act in, by id or slug; left out, the target says. */
  workspaceId?: string
  /** A write's request id: a retry with the same one is answered once. */
  requestId?: string
  /** Run it and roll it back (operations that have a preview). */
  preview?: boolean
  /** The workspace on screen when the call was first made, for its retries. */
  onScreen?: string
  signal?: AbortSignal
}

/** Run an operation. A refusal throws `ApiError` with the outcome as its code. */
export async function callOp<N extends OpName>(
  name: N,
  input: Operations[N]['input'],
  options: CallOptions = {},
): Promise<Operations[N]['output']> {
  const base = options.workspaceId
    ? `/api/v2/w/${encodeURIComponent(options.workspaceId)}/ops/${name}`
    : `/api/v2/ops/${name}`
  // The workspace on screen: where a call that names none acts, when its
  // target doesn't say and the person reaches several.
  const onScreen = options.onScreen ?? currentWorkspaceSlug()
  const envelope = await requestJson<Envelope<Operations[N]['output']>>(
    options.preview ? `${base}?preview=true` : base,
    {
      method: 'POST',
      headers: onScreen === CURRENT_WORKSPACE ? undefined : { 'x-current-workspace': onScreen },
      json: input,
      idempotencyKey: options.requestId,
      signal: options.signal,
    },
  )
  if (envelope.outcome !== 'ok') {
    throw new ApiError({
      status: 200,
      code: envelope.outcome,
      message: envelope.message,
      details: { ...envelope.details, field: envelope.field, reason: envelope.code },
    })
  }
  return envelope.value
}
