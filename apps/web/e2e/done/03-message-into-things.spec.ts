import { randomUUID } from 'node:crypto'

import { expect, test, type Page } from '@playwright/test'

import { seededValue, storageStatePath } from '../environment'
import { workspaceId, writer } from '../support/done'

// Test for "done", item 3: talk in a channel, and turn a message into a
// task, a decision, a page or a whiteboard without leaving the conversation.
// Each comes from the message's More menu, opens a dialog over the channel,
// and the channel is still there afterwards.

test.use({ storageState: storageStatePath })

async function fromMenu(page: Page, text: string, item: RegExp) {
  const message = page.getByRole('article').filter({ hasText: text }).last()
  await message.hover()
  await message.getByRole('toolbar', { name: 'Message actions' }).getByRole('button', { name: /more/i }).click()
  await page.getByRole('menuitem', { name: item }).click()
  return page.getByRole('dialog')
}

test('a channel message becomes a task, a decision, a page and a sticky on a whiteboard', async ({ page, context }) => {
  const channel = `/projects/${seededValue('E2E_PROJECT_ID')}/channels/${seededValue('E2E_CHANNEL_ID')}`
  await page.goto(channel)
  const { request, headers } = await writer(context, page)
  const text = `Ship the export fix before Friday ${randomUUID().slice(0, 6)}`
  const sent = await request.post(`/api/v2/w/${await workspaceId(request)}/ops/message_send`, {
    headers: headers(randomUUID()),
    data: { conversationId: seededValue('E2E_CHANNEL_ID'), text },
  })
  expect(sent.ok(), await sent.text()).toBe(true)
  await expect(page.getByText(text).first()).toBeVisible()
  const here = page.url()

  let dialog = await fromMenu(page, text, /^Create task/)
  await dialog.getByRole('button', { name: /^Create task/ }).click()
  await expect(dialog).toBeHidden()

  dialog = await fromMenu(page, text, /^Save as decision/)
  await dialog.getByRole('button', { name: /^(Save|Record) decision/ }).click()
  await expect(dialog).toBeHidden()

  dialog = await fromMenu(page, text, /^Make a page/)
  await dialog.getByRole('button', { name: /^Make page/ }).click()
  await expect(dialog).toBeHidden()

  dialog = await fromMenu(page, text, /^Put on a whiteboard/)
  await dialog.getByRole('button', { name: 'Put on whiteboard' }).click()
  await expect(dialog).toBeHidden()

  // Still in the conversation, and the message says what it was made into:
  // each thing is there to open, now and when the channel is opened again.
  expect(page.url()).toBe(here)
  await page.reload()
  const message = page.getByRole('article').filter({ hasText: text }).last()
  await expect(message).toBeVisible()
  for (const thing of ['task', 'decision', 'page', 'sticky']) {
    await expect(message.getByText(new RegExp(`Made into an? ${thing}:`)), `the ${thing} it was made into`).toBeVisible()
  }
})
