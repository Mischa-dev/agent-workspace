import path from 'node:path'

import { expect, test } from '@playwright/test'

import { screenshotDir, seededValue, storageStatePath } from './environment'

test.use({ storageState: storageStatePath })

// The first page to load saves this browser's appearance to the account, and
// later specs expect "follow the system", so start from that.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.setItem(
      'agent-workspace:appearance:v2',
      JSON.stringify({ theme: 'system', overrides: {}, density: 'comfortable', textSize: 'default' }),
    )
  })
})

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
    // On a touch screen Enter makes a new line; the button sends.
    await page.getByRole('button', { name: 'Send message' }).click()
    const message = page.locator('.aw-msg', { hasText: 'From my phone' }).last()
    await expect(message.getByRole('button', { name: 'Open phone.png' })).toBeVisible()
    await message.scrollIntoViewIfNeeded()
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)
    expect(overflow).toBeLessThanOrEqual(0)
    await page.screenshot({ path: path.join(screenshotDir, 'files-message-phone.png') })
  })
})

// A task (issue) gets files when it's made, from its Files section, and on a
// comment; all of them show on the task, at a desk and on a phone.
for (const device of [
  { name: 'desktop', use: { viewport: { width: 1280, height: 800 } } },
  { name: 'phone', use: { viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true } },
]) {
  test.describe(`task files on ${device.name}`, () => {
    test.use(device.use)

    test('attaches files to a task as it is made, later, and on a comment', async ({ page }) => {
      const errors: string[] = []
      page.on('pageerror', (error) => errors.push(error.message))
      await page.goto(`/projects/${seededValue('E2E_PROJECT_ID')}/work-items`)
      await page.getByRole('button', { name: 'New task' }).first().click()
      const dialog = page.getByRole('dialog', { name: 'New task' })
      await dialog.getByLabel('Title').fill(`Files on ${device.name}`)
      await dialog.locator('input[type="file"]').setInputFiles([{ name: 'spec.png', mimeType: 'image/png', buffer: PNG }])
      await expect(dialog.getByRole('list', { name: 'Files to attach' }).getByText('spec.png')).toBeVisible()
      await page.screenshot({ path: path.join(screenshotDir, `task-files-new-${device.name}.png`) })
      await dialog.getByRole('button', { name: 'Create task' }).click()
      await expect(dialog).toBeHidden()

      const filesSection = page.locator('section.aw-worksection', { has: page.getByRole('heading', { name: 'Files' }) })
      await expect(filesSection.getByRole('button', { name: 'Open spec.png' })).toBeVisible()
      await filesSection.locator('input[type="file"]').setInputFiles([
        { name: 'later.txt', mimeType: 'text/plain', buffer: Buffer.from('added later') },
      ])
      await expect(filesSection.getByRole('link', { name: 'Download later.txt' })).toBeVisible()

      const comment = page.getByRole('combobox', { name: 'Add a comment' })
      const composer = page.locator('.aw-composer', { has: comment })
      await composer.locator('input[type="file"]').setInputFiles([
        { name: 'log.txt', mimeType: 'text/plain', buffer: Buffer.from('a log') },
      ])
      await expect(composer.locator('.aw-upload.is-done')).toHaveCount(1)
      await comment.fill('Here is the log')
      // On a touch screen Enter makes a new line; the button sends.
      if (device.name === 'phone') await composer.getByRole('button', { name: 'Send message' }).click()
      else await comment.press('Enter')
      const posted = page.locator('.aw-msg, li, article', { hasText: 'Here is the log' }).last()
      await expect(posted.getByRole('link', { name: 'Download log.txt' })).toBeVisible()
      await filesSection.scrollIntoViewIfNeeded()
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)
      expect(overflow).toBeLessThanOrEqual(0)
      await page.screenshot({ path: path.join(screenshotDir, `task-files-${device.name}.png`), fullPage: true })
      expect(errors, 'uncaught errors on the page').toEqual([])
    })
  })
}
