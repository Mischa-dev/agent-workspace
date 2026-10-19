import { expect, test } from '@playwright/test'

import { seeded, seededValue } from './environment'

// The flows that many parallel PRs could break without any unit test noticing.
// Each one runs in a fresh, signed-out browser.

test('signs in with a password and lands on Home', async ({ page }) => {
  await page.goto('/')
  await expect(page).toHaveURL(/\/sign-in/)
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible()

  await page.getByLabel('Username').fill(seeded.loginName)
  await page.getByLabel('Password').fill(seededValue('E2E_PASSWORD'))
  await page.getByRole('button', { name: 'Sign in' }).click()

  await expect(page).toHaveURL(/\/home$/)
  await expect(page.getByRole('heading', { name: new RegExp(`^(Welcome|Good \\w+), ${seeded.displayName.split(' ')[0]}$`) })).toBeVisible()
  await expect(
    page.getByRole('navigation', { name: 'Spaces' }).getByRole('link', { name: seeded.projectName }),
  ).toBeVisible()
})

test('rejects a wrong password', async ({ page }) => {
  await page.goto('/sign-in')
  await page.getByLabel('Username').fill(seeded.loginName)
  await page.getByLabel('Password').fill('not-the-right-password')
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page.getByRole('alert').first()).toBeVisible()
  await expect(page).toHaveURL(/\/sign-in/)
})

test('creates a task and moves it across the board', async ({ page }) => {
  const title = `Smoke task ${Date.now()}`

  await page.goto('/sign-in')
  await page.getByLabel('Username').fill(seeded.loginName)
  await page.getByLabel('Password').fill(seededValue('E2E_PASSWORD'))
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page).toHaveURL(/\/home$/)

  // Into the project and its tasks, the way a person would get there.
  // The rail reopens the last place this person was in the Project, which
  // another spec may have left on its settings, so start from the channel.
  await page.goto(`/projects/${seededValue('E2E_PROJECT_ID')}/channels/${seededValue('E2E_CHANNEL_ID')}`)
  await page.getByRole('link', { name: 'Tasks', exact: true }).first().click()
  await expect(page.getByRole('heading', { name: 'Tasks', exact: true })).toBeVisible()

  await page.getByRole('button', { name: 'New task' }).first().click()
  const dialog = page.getByRole('dialog', { name: 'New task' })
  await dialog.getByLabel('Title').fill(title)
  await dialog.getByLabel('Type').selectOption({ label: 'Task' })
  await dialog.getByRole('button', { name: 'Create task' }).click()

  // Creating opens the new item in the task panel beside the list; the URL
  // carries it, so the panel survives a reload or a shared link.
  await expect(page).toHaveURL(/[?&]task=[^&]+/)
  const heading = page.getByRole('heading', { name: new RegExp(`^${seeded.projectKey}-\\d+: ${title}$`) })
  await expect(heading).toHaveCount(1)
  // Its key is the Project key plus a number. Read it off the panel, since
  // the seeded item sits on the same board.
  const key = (await heading.textContent())?.split(':')[0]
  expect(key).toMatch(new RegExp(`^${seeded.projectKey}-\\d+$`))

  await page.goto(`/projects/${seededValue('E2E_PROJECT_ID')}/work-items`)
  await page.getByRole('radiogroup', { name: 'View' }).getByRole('radio', { name: 'Board' }).click()
  await expect(page).toHaveURL(/view=board/)
  const board = page.getByRole('region', { name: 'Task board' })
  const column = (name: string) => board.getByRole('region', { name, exact: true })
  const card = (name: string) => column(name).getByRole('listitem', { name: `${key} ${title}` })

  // New items start in Backlog.
  await expect(card('Backlog')).toBeVisible()

  // Move with the keyboard: Space picks the card up, an arrow carries it to
  // the next column, Space drops it. Backlog → Ready.
  // Each step waits for the board's screen-reader announcement, as a person
  // would wait to see the card lift and move.
  const announced = (text: string) => expect(page.getByText(text, { exact: true })).toBeAttached()
  await card('Backlog').focus()
  await page.keyboard.press('Space')
  await announced(`Picked up ${key}. Arrow keys move it, Space drops it, Escape puts it back.`)
  await page.keyboard.press('ArrowRight')
  await announced('Ready, position 1.')
  await page.keyboard.press('Space')
  await expect(card('Ready')).toBeVisible()
  await expect(card('Backlog')).toHaveCount(0)

  // And by dragging the card: Ready → In Progress.
  await card('Ready').dragTo(column('In Progress'), { steps: 10 })
  await expect(card('In Progress')).toBeVisible()
  await expect(card('Ready')).toHaveCount(0)

  // The move is stored on the server, not just shown: it survives a reload.
  await page.reload()
  await expect(card('In Progress')).toBeVisible()
})
