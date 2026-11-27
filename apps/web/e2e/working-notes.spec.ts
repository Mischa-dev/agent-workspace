import { randomUUID } from 'node:crypto'
import path from 'node:path'

import { expect, test } from '@playwright/test'

import { screenshotDir, seededValue, storageStatePath, webOrigin } from './environment'
import { asAgent, workspaceId, writer } from './support/done'

// AWD-220: two agents settle who touches which files in working notes. The
// person in the channel sees one folded line, not the back-and-forth, isn't
// shown anything as unread, and can open the fold to read every word.

test.use({ storageState: storageStatePath })

test('agents’ working notes fold to one line for the person reading', async ({ page, context, playwright }) => {
  const space = seededValue('E2E_PROJECT_ID')
  const channel = seededValue('E2E_CHANNEL_ID')
  const address = `/projects/${space}/channels/${channel}`
  await page.goto(address)
  const bare = await playwright.request.newContext({ baseURL: webOrigin, storageState: { cookies: [], origins: [] } })
  const { request, headers } = await writer(context, page)
  const workspace = await workspaceId(request)
  const tag = randomUUID().slice(0, 6)
  const agent = async (name: string) => {
    const made = await request.post(`/api/v1/projects/${space}/external-agents`, {
      headers: headers(),
      data: { connectionName: `${name} runner`, displayName: name, handle: name.toLowerCase(), role: 'member', scopes: ['read', 'write'] },
    })
    expect(made.ok(), await made.text()).toBe(true)
    const token = ((await made.json()) as { oneTimeCredential: { token: string } }).oneTimeCredential.token
    return asAgent(bare, token)
  }
  const say = async (client: ReturnType<typeof asAgent>, input: Record<string, unknown>) => {
    const answer = await client.call('message_send', { conversationId: channel, ...input }, workspace)
    expect(answer.outcome, JSON.stringify(answer)).toBe('ok')
  }
  const mara = await agent(`Mara${tag}`)
  const rook = await agent(`Rook${tag}`)

  await say(mara, { text: `Starting on the importer ${tag}.` })
  await say(mara, { note: true, text: `I have importer/parse.rs and its tests ${tag}.` })
  await say(rook, { note: true, text: `Then I take importer/write.rs ${tag}. Leave Cargo.toml to me.` })
  await say(mara, { note: true, text: `Agreed ${tag}.` })
  await say(rook, { text: `Both halves are up for review ${tag}.` })

  await page.goto(address)
  await expect(page.getByText(`Both halves are up for review ${tag}.`)).toBeVisible()
  const fold = page.getByRole('button', { name: new RegExp(`3 working notes.*Mara${tag} and Rook${tag}`) })
  await expect(fold).toBeVisible()
  await expect(page.getByText(`I have importer/parse.rs and its tests ${tag}.`)).toHaveCount(0)
  if (process.env.E2E_LOOK) {
    for (const scheme of ['light', 'dark'] as const) {
      await page.emulateMedia({ colorScheme: scheme })
      await page.screenshot({ path: path.join(screenshotDir, `working-notes-folded-${scheme}.png`) })
    }
  }

  await fold.click()
  await expect(page.getByText(`I have importer/parse.rs and its tests ${tag}.`)).toBeVisible()
  await expect(page.getByText(`Agreed ${tag}.`)).toBeVisible()
  if (process.env.E2E_LOOK) {
    for (const scheme of ['dark', 'light'] as const) {
      await page.emulateMedia({ colorScheme: scheme })
      await page.screenshot({ path: path.join(screenshotDir, `working-notes-open-${scheme}.png`) })
    }
    await page.setViewportSize({ width: 390, height: 844 })
    await page.goto(address)
    await expect(page.getByRole('button', { name: /3 working notes/ })).toBeVisible()
    await page.screenshot({ path: path.join(screenshotDir, 'working-notes-folded-phone.png') })
    await page.getByRole('button', { name: /3 working notes/ }).click()
    await page.emulateMedia({ colorScheme: 'dark' })
    await page.screenshot({ path: path.join(screenshotDir, 'working-notes-open-phone-dark.png') })
  }
})
