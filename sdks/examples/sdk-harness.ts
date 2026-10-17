// A connected runner that takes a task to a pull request, with the
// TypeScript SDK. Tag the agent with a task; it makes a branch in a checkout,
// runs your agent on it, commits, pushes and opens the PR with `gh`, and
// replies with the link. Stop kills the agent.
//
//   AW_EXAMPLE_REPO=/srv/checkouts/app \
//   AW_EXAMPLE_AGENT='["/usr/local/bin/my-agent", "--yes"]' \
//   node sdk-harness.ts        (plus the AGENT_WORKSPACE_* runner settings)
//
// The agent gets the prompt on stdin and a bare environment: never the
// runner's token or webhook secret.

import { execFile, spawn } from 'node:child_process'
import { promisify } from 'node:util'
import { Runner, runnerSettingsFromEnv, type RequestContext } from '../runner-typescript/src/index.ts'

const run = promisify(execFile)
const repo = process.env.AW_EXAMPLE_REPO ?? ''
const agent: string[] = JSON.parse(process.env.AW_EXAMPLE_AGENT ?? '[]')
if (!repo || !agent.length) throw new Error('set AW_EXAMPLE_REPO and AW_EXAMPLE_AGENT')

const git = async (...args: string[]) => (await run('git', args, { cwd: repo })).stdout.trim()

/** Runs the agent in the checkout until it exits or the signal aborts. */
function runAgent(prompt: string, signal: AbortSignal): Promise<number | null> {
  return new Promise((resolve, reject) => {
    const child = spawn(agent[0], agent.slice(1), {
      cwd: repo,
      env: { PATH: '/usr/local/bin:/usr/bin:/bin', HOME: process.env.HOME ?? '/', LANG: 'C.UTF-8' },
      stdio: ['pipe', 'ignore', 'ignore'],
      detached: true, // its own process group, so Stop reaches whatever it started
    })
    const stop = () => child.pid && process.kill(-child.pid, 'SIGTERM')
    signal.addEventListener('abort', stop, { once: true })
    child.on('error', reject)
    child.on('exit', (code) => {
      signal.removeEventListener('abort', stop)
      resolve(code)
    })
    child.stdin.end(prompt)
  })
}

async function takeToPullRequest(request: RequestContext) {
  const branch = `runner/${request.request.requestId.slice(-12)}`
  const base = await git('rev-parse', '--abbrev-ref', 'HEAD')
  await git('switch', '-c', branch)
  try {
    await request.bindSession(branch, { state: 'active' })
    request.onFollowup((followup) => void followup.pass('Ask again once the pull request is up.').catch(() => {}))
    await request.progress(`Running the agent on ${branch}`)
    const code = await runAgent(request.prompt, request.signal)
    if (request.signal.aborted) return
    if (code !== 0) return await request.release(`The agent exited with ${code}.`)
    if (!(await git('status', '--porcelain'))) return await request.reply('The agent finished without changing anything.')
    await request.progress('Committing and pushing')
    await git('add', '-A')
    await git('commit', '-m', request.prompt.split('\n')[0].slice(0, 72) || 'Agent change')
    await git('push', '-u', 'origin', branch)
    await request.progress('Opening the pull request')
    const title = request.prompt.split('\n')[0].slice(0, 72) || branch
    const body = `${request.prompt}\n\nOpened by an Agent Workspace runner.`
    const { stdout } = await run('gh', ['pr', 'create', '--head', branch, '--title', title, '--body', body], { cwd: repo })
    await request.reply(`Opened ${stdout.trim().split('\n').at(-1)}`)
  } finally {
    await request.bindSession(branch, { state: 'idle' }).catch(() => {})
    // A checkout kept for the runner: drop whatever a stopped agent left.
    await git('switch', '--force', base).catch(() => '')
    await git('clean', '-fd').catch(() => '')
  }
}

// One checkout, so one task at a time; the rest wait their turn.
let queue: Promise<void> = Promise.resolve()
const runner = new Runner({
  ...runnerSettingsFromEnv(),
  onRequest(request) {
    const turn = queue.then(() => (request.signal.aborted ? undefined : takeToPullRequest(request)))
    queue = turn.catch(() => {})
    return turn
  },
})

if (process.env.AGENT_WORKSPACE_WEBHOOK_SECRET) runner.listen(Number(process.env.PORT ?? 8787))
else void runner.poll()
