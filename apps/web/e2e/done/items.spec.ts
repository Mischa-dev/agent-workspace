import { test } from '@playwright/test'

// Test for "done" (END-STATE section 1): the items whose parts don't exist
// yet, each a fixme naming the workstreams that own it. An item moves into
// its own spec (`done/NN-<name>.spec.ts`) when they build it; item 8 has one.

const OWED: ReadonlyArray<readonly [owners: string, item: string]> = [
  ['H1, K4', '01 sign up, name a workspace and invite a teammate'],
  ['A2', '02 add an agent three ways: the directory, a pasted link, a linked host'],
  ['C2, C1, T1', '03 in a channel, turn a message into a task, a decision, a page or a whiteboard'],
  ['C1, A2', '04 on a phone, an agent edits a sheet live, with its name on the edit and one tap to undo'],
  ['K1', '05 give an agent a new hire’s access, or your own, by picking a role'],
  ['T1, M1', '06 a tracker with its own fields, a board and a public form, triaged by an agent'],
  ['A1', '07 see what every agent is doing, what it cost and what it waits on, in one place'],
  ['H1', '09 export the whole workspace and import it on another server'],
  ['K4, A2', '10 add your own agent to a second workspace in one step, and DM it once'],
  ['M1, C2', '11 set one up with friends only to chat, and never see a task, a project or an agent'],
]

for (const [owners, item] of OWED) {
  test.fixme(item, { annotation: { type: 'owner', description: owners } }, async () => {})
}
