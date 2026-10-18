import path from 'node:path'

import { expect, test } from '@playwright/test'

import { screenshotDir, seededValue, storageStatePath } from './environment'

test.use({ storageState: storageStatePath })

/** A 2x2 PNG, red. */
const PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4z8AARAwQCgAf7gP9i18U1AAAAABJRU5ErkJggg==',
  'base64',
)

// A message with a picture, a file and Markdown, sent from the composer, as
// someone on a phone and at a desk sees it.
test('sends a picture and a file with formatted text, and opens the picture', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  const channel = `/projects/${seededValue('E2E_PROJECT_ID')}/channels/${seededValue('E2E_CHANNEL_ID')}`
  await page.goto(channel)
  const box = page.getByRole('combobox', { name: /Message/ })
  await expect(box).toBeVisible()

  await page.locator('.aw-composer input[type="file"]').setInputFiles([
    { name: 'red.png', mimeType: 'image/png', buffer: PNG },
    { name: 'notes.md', mimeType: 'text/markdown', buffer: Buffer.from('# Notes\n') },
  ])
  const chips = page.getByRole('list', { name: 'Attached files' })
  await expect(chips.getByText('red.png')).toBeVisible()
  await expect(chips.getByText('notes.md')).toBeVisible()
  await expect(chips.locator('.aw-upload.is-done')).toHaveCount(2)

  await box.fill('Release plan\n- ship the **files**\n- check `link previews`')
  await page.getByRole('button', { name: 'Formatting' }).click()
  await expect(page.getByRole('toolbar', { name: 'Formatting' })).toBeVisible()
  await box.press('Enter')

  const message = page.locator('.aw-msg', { hasText: 'Release plan' }).last()
  await expect(message.locator('li')).toHaveCount(2)
  await expect(message.locator('strong')).toHaveText('files')
  await expect(message.getByRole('link', { name: 'Download notes.md' })).toBeVisible()
  const thumbnail = message.getByRole('button', { name: 'Open red.png' })
  await expect(thumbnail).toBeVisible()
  await expect(thumbnail.locator('img')).toHaveJSProperty('complete', true)
  await page.screenshot({ path: path.join(screenshotDir, 'files-message.png') })

  await thumbnail.click()
  const viewer = page.getByRole('dialog', { name: 'red.png' })
  await expect(viewer).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(viewer).toBeHidden()

  // The file opens for this session and downloads with its name.
  const fileLink = await message.getByRole('link', { name: 'Download notes.md' }).getAttribute('href')
  const download = await page.request.get(fileLink ?? '')
  expect(download.status()).toBe(200)
  expect(download.headers()['content-disposition']).toContain('notes.md')
  expect(errors, 'uncaught errors on the page').toEqual([])
})

test.describe('on a phone', () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true })

  test('attaches a picture and sends it without the page overflowing', async ({ page }) => {
    const channel = `/projects/${seededValue('E2E_PROJECT_ID')}/channels/${seededValue('E2E_CHANNEL_ID')}`
    await page.goto(channel)
    const box = page.getByRole('combobox', { name: /Message/ })
    await expect(box).toBeVisible()
    await page.locator('.aw-composer input[type="file"]').setInputFiles([
      { name: 'phone.png', mimeType: 'image/png', buffer: PNG },
      { name: 'a-report-with-a-long-name-from-the-phone.pdf', mimeType: 'application/pdf', buffer: Buffer.from('%PDF-1.4\n') },
    ])
    await expect(page.getByRole('list', { name: 'Attached files' }).locator('.aw-upload.is-done')).toHaveCount(2)
    await box.fill('From my phone')
    await page.screenshot({ path: path.join(screenshotDir, 'files-composer-phone.png') })
    await box.press('Enter')
    const message = page.locator('.aw-msg', { hasText: 'From my phone' }).last()
    await expect(message.getByRole('button', { name: 'Open phone.png' })).toBeVisible()
    await message.scrollIntoViewIfNeeded()
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)
    expect(overflow).toBeLessThanOrEqual(0)
    await page.screenshot({ path: path.join(screenshotDir, 'files-message-phone.png') })
  })
})
