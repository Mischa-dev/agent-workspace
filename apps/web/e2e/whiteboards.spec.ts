import { expect, test, type Page } from '@playwright/test'

import { storageStatePath } from './environment'

test.use({ storageState: storageStatePath })

// One board open in two tabs: what one person draws reaches the other live,
// and the drawing is still there after a reload.
test('draws on a whiteboard and the other tab sees it live', async ({ browser, page }) => {
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  const title = `Smoke board ${Date.now()}`

  await page.goto('/whiteboards')
  await expect(page.getByRole('heading', { name: 'Whiteboards', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'New whiteboard' }).first().click()
  const dialog = page.getByRole('dialog', { name: 'New whiteboard' })
  await dialog.getByLabel('Title').fill(title)
  await dialog.getByRole('button', { name: 'Create board' }).click()
  await expect(page).toHaveURL(/\/whiteboards\/[^/]+$/)
  const boardId = page.url().split('/').at(-1) ?? ''
  await expect(canvas(page)).toBeVisible()

  const other = await browser.newContext({ storageState: storageStatePath })
  const watcher = await other.newPage()
  await watcher.goto(`/whiteboards/${boardId}`)
  await expect(canvas(watcher)).toBeVisible()
  // The second tab learns of the change from the live stream and pulls it.
  const pulled = watcher.waitForResponse(
    async (response) =>
      response.url().includes(`/api/v1/whiteboards/${boardId}/scene?since=`) &&
      ((await response.json()) as { elements: unknown[] }).elements.length > 0,
  )

  await drawRectangle(page)
  await expect(page.getByText('All changes saved')).toBeVisible()
  await pulled

  await page.reload()
  await expect(canvas(page)).toBeVisible()
  const scene = await page.request.get(`/api/v1/whiteboards/${boardId}/scene`)
  const elements = ((await scene.json()) as { elements: Array<{ type: string; isDeleted: boolean }> }).elements
  expect(elements.filter((element) => element.type === 'rectangle' && !element.isDeleted)).toHaveLength(1)
  expect(errors, 'uncaught errors on the page').toEqual([])
  await other.close()
})

const canvas = (page: Page) => page.locator('canvas.excalidraw__canvas.interactive')

async function drawRectangle(page: Page) {
  const box = await canvas(page).boundingBox()
  if (!box) throw new Error('The canvas has no size')
  await canvas(page).click({ position: { x: box.width / 2, y: box.height / 2 } })
  await page.keyboard.press('r')
  await page.mouse.move(box.x + box.width / 2 - 100, box.y + box.height / 2 - 60)
  await page.mouse.down()
  await page.mouse.move(box.x + box.width / 2 + 100, box.y + box.height / 2 + 60, { steps: 8 })
  await page.mouse.up()
}
