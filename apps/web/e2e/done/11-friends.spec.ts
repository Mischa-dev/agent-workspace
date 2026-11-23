import { expect, test } from '@playwright/test'

import { storageStatePath } from '../environment'
import { secondWorkspace } from '../support/done'

// Test for "done", item 11: set one up with friends only to chat, and never
// see a task, a project or an agent. A workspace of its own (imported, so
// the other specs' workspaces keep their shape) is set up as Friends from
// Settings, and then nothing in it names tasks, projects or agents.

test.use({ storageState: storageStatePath })

test('a workspace set up as Friends shows chat and nothing about tasks, projects or agents', async ({ page, context }, info) => {
  const friends = await secondWorkspace(context, page, `friends-${info.project.name}`)
  await page.goto(`/w/${friends.slug}/settings/modules`)
  const shapes = page.getByRole('list', { name: 'What this workspace is for' })
  await expect(shapes.getByRole('heading', { name: 'Friends' })).toBeVisible()
  const friendsShape = shapes.getByRole('listitem').filter({ has: page.getByRole('heading', { name: 'Friends' }) })
  if (!(await friendsShape.getByText('This workspace').isVisible())) {
    await friendsShape.getByRole('button', { name: 'Choose Friends' }).click()
    await page.getByRole('button', { name: 'Set up as Friends' }).click()
    await expect(friendsShape.getByText('This workspace')).toBeVisible()
  }

  for (const address of ['home', 'messages']) {
    await page.goto(`/w/${friends.slug}/${address}`)
    await page.waitForLoadState('networkidle')
    const main = page.locator('#main, main').first()
    await expect(main).toBeVisible()
    // Anywhere on the page, shown or in a closed drawer.
    for (const word of [/\btasks?\b/i, /\bprojects?\b/i, /\bagents?\b/i]) {
      const named = page.getByRole('link', { name: word, includeHidden: true })
      await expect(named, `${address} links to ${word}`).toHaveCount(0)
      await expect(page.getByRole('button', { name: word, includeHidden: true }), `${address} offers ${word}`).toHaveCount(0)
    }
  }
  await page.goto(`/w/${friends.slug}/messages`)
  // Starting a conversation is right there: a button at a desk, the new-message screen itself on a phone.
  await expect(
    page.getByRole('button', { name: /new message/i }).or(page.getByRole('heading', { name: 'New message' })).first(),
  ).toBeVisible()
})
