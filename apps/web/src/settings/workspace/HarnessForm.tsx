import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState, type FormEvent } from 'react'

import { isApiError } from '../../api/errors'
import { Banner, Button, Dialog, SegmentedControl, SelectField, TextArea, TextField, showToast } from '../../ui'
import {
  STARTER_MANIFEST,
  createHarness,
  harnessKeys,
  parseManifest,
  splitLines,
  updateHarness,
  type CustomHarness,
  type HarnessList,
  type HarnessTransport,
} from './harnessesApi'

/** The server's reason, or a plain one when its refusal carries none we can read. */
function refusalText(error: unknown) {
  if (isApiError(error) && error.status === 422 && error.code === 'request_failed') {
    return 'The server refused this harness. Check the manifest, the command and the runner.'
  }
  return error instanceof Error ? error.message : 'Try again.'
}

/** Registers a harness, or edits one. Its transport can't change once registered. */
export function HarnessForm({
  harness,
  list,
  onClose,
  organizationId,
}: {
  harness: CustomHarness | null
  list: HarnessList
  onClose: () => void
  organizationId: string
}) {
  const client = useQueryClient()
  const [name, setName] = useState(harness?.name ?? '')
  const [transport, setTransport] = useState<HarnessTransport>(harness?.transport ?? (list.commandsAllowed ? 'command' : 'runner'))
  const [manifest, setManifest] = useState(harness ? JSON.stringify(harness.manifest, null, 2) : STARTER_MANIFEST)
  const [command, setCommand] = useState(harness?.command?.join('\n') ?? '')
  const [secrets, setSecrets] = useState(harness?.secretNames.join(', ') ?? '')
  const [runner, setRunner] = useState(harness?.runnerActorId ?? '')
  const [problem, setProblem] = useState<string | null>(null)

  const save = useMutation({
    mutationFn: async () => {
      const parsed = parseManifest(manifest)
      if ('error' in parsed) throw new Error(parsed.error)
      const pieces =
        transport === 'command'
          ? { command: splitLines(command), secretNames: splitLines(secrets, true) }
          : { runnerActorId: runner || undefined }
      if (harness) return updateHarness(organizationId, harness.id, { version: harness.version, name: name.trim(), manifest: parsed.value, ...pieces })
      return createHarness(organizationId, { name: name.trim(), transport, manifest: parsed.value, ...pieces })
    },
    onSuccess: (saved) => {
      void client.invalidateQueries({ queryKey: harnessKeys.list(organizationId) })
      showToast({ tone: 'success', title: harness ? `${saved.name} saved` : `${saved.name} registered` })
      onClose()
    },
    onError: (error) => setProblem(refusalText(error)),
  })

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setProblem(null)
    save.mutate()
  }

  const commandBlocked = transport === 'command' && !list.commandsAllowed
  const missing = !name.trim() || (transport === 'command' ? splitLines(command).length === 0 : !runner)

  return (
    <Dialog
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={save.isPending} disabled={missing || commandBlocked} type="submit" variant="primary">
            {harness ? 'Save' : 'Register'}
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={submit}
      open
      title={harness ? `Edit ${harness.name}` : 'Register a harness'}
      width={640}
    >
      <div className="aw-harnessform">
        {problem ? (
          <Banner tone="danger" title="Not saved.">
            {problem}
          </Banner>
        ) : null}
        <TextField autoFocus label="Name" maxLength={80} onChange={(event) => setName(event.target.value)} required value={name} />
        {harness ? null : (
          <SegmentedControl
            label="How it runs"
            onChange={setTransport}
            options={[
              { value: 'command', label: 'Command on this server' },
              { value: 'runner', label: 'Connected runner' },
            ]}
            value={transport}
          />
        )}
        {commandBlocked ? <CommandsOffNote /> : null}
        {transport === 'command' ? (
          <>
            <TextArea
              className="aw-harnessform__code"
              hint="One argument per line, starting with the program. It must sit in a folder the server owner allowed. Use {model}, {reasoning}, {speed} or {option:NAME} for launch settings."
              label="Command"
              onChange={(event) => setCommand(event.target.value)}
              rows={4}
              value={command}
            />
            <TextField
              hint="Workspace keys the command may read, separated by commas. Their values never appear in its environment or arguments."
              label="Keys it reads"
              onChange={(event) => setSecrets(event.target.value)}
              value={secrets}
            />
          </>
        ) : (
          <SelectField
            hint={list.runners.length === 0 ? 'No agent has a runner attached yet. Attach one on the Agents page first.' : 'The agent whose runner takes these launches.'}
            label="Runner"
            onChange={(event) => setRunner(event.target.value)}
            value={runner}
          >
            <option value="">Pick an agent</option>
            {list.runners.map((choice) => (
              <option key={choice.actorId} value={choice.actorId}>
                {choice.displayName}
              </option>
            ))}
          </SelectField>
        )}
        <TextArea
          className="aw-harnessform__code"
          hint="Name, description, models, options, features and how many runs an account may have at once."
          label="Manifest (JSON)"
          onChange={(event) => setManifest(event.target.value)}
          rows={12}
          spellCheck={false}
          value={manifest}
        />
      </div>
    </Dialog>
  )
}

export function CommandsOffNote() {
  return (
    <Banner tone="info" title="Command harnesses are off on this server.">
      The server owner turns them on by setting <code>AGENT_WORKSPACE_COMMAND_HARNESS_DIRS</code> to the folders their programs may run from. Connected runners
      work without it.
    </Banner>
  )
}
