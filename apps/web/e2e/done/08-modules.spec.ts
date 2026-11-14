import { expect, test } from '@playwright/test'

import { seeded, seededValue, storageStatePath } from '../environment'
import { setModules } from '../support/done'

// Test for "done", item 8: switch off everything except chat, or everything
// except pages, and have the app still feel whole. Owner: M1.

test.use({ storageState: storageStatePath })

test('everything off but chat: the project opens on its channels, with nothing else in its sidebar', async ({ page, context }) => {
  const channel = `/projects/${seededValue('E2E_PROJECT_ID')}/channels/${seededValue('E2E_CHANNEL_ID')}`
  await page.goto(channel)
  const modules = await setModules(context, page)
  try {
    await modules.only(['messages', 'projects', 'channels', 'agents', 'inbox'])
    await page.goto(channel)
    // On a phone the sidebar sits in a closed drawer: it's there, not shown,
    // so the links are looked for hidden ones included.
    const sidebar = page.locator('#project-sidebar')
    await expect(sidebar.getByRole('link', { name: seeded.channelName, includeHidden: true })).toBeAttached()
    for (const row of ['Tasks', 'Pages', 'Decisions', 'Collections', 'Sheets', 'Runs']) {
      await expect(sidebar.getByRole('link', { name: row, exact: true, includeHidden: true })).toHaveCount(0)
    }
  } finally {
    await modules.restore()
  }
})

test.fixme(
  'everything off but pages: pages open and nothing asks for chat',
  { annotation: { type: 'owner', description: 'M1 (pages-only shape, M1.17), K4 (the workspace space)' } },
  async () => {},
)
