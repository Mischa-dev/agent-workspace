import { expect, test } from '@playwright/test'

import { seeded, seededValue } from './environment'

// The flows that many parallel PRs could break without any unit test noticing.
// Each one runs in a fresh, signed-out browser.

test('signs in with a password and lands on Home', async ({ page }) => {
  await page.goto('/')
  await expect(page).toHaveURL(/\/sign-in/)
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible()

  await page.getByLabel('Login name').fill(seeded.loginName)
  await page.getByLabel('Password').fill(seededValue('E2E_PASSWORD'))
  await page.getByRole('button', { name: 'Sign in' }).click()

  await expect(page).toHaveURL(/\/home$/)
  await expect(page.getByRole('heading', { name: `Welcome back, ${seeded.displayName}` })).toBeVisible()
  await expect(
    page.getByRole('navigation', { name: 'Projects' }).getByRole('link', { name: seeded.projectName }),
  ).toBeVisible()
})

test('rejects a wrong password', async ({ page }) => {
  await page.goto('/sign-in')
  await page.getByLabel('Login name').fill(seeded.loginName)
  await page.getByLabel('Password').fill('not-the-right-password')
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page.getByRole('alert').first()).toBeVisible()
  await expect(page).toHaveURL(/\/sign-in/)
})

test('creates a task and moves it across the board', async ({ page }) => {
  const title = `Smoke task ${Date.now()}`

  await page.goto('/sign-in')
  await page.getByLabel('Login name').fill(seeded.loginName)
  await page.getByLabel('Password').fill(seededValue('E2E_PASSWORD'))
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page).toHaveURL(/\/home$/)

  // Into the Project and its work items, the way a person would get there.
  await page.getByRole('navigation', { name: 'Projects' }).getByRole('link', { name: seeded.projectName }).click()
  await page.getByRole('link', { name: 'Work items' }).click()
  await expect(page.getByRole('heading', { name: 'Work items', exact: true })).toBeVisible()

  await page.getByRole('button', { name: 'New work item' }).first().click()
  const dialog = page.getByRole('dialog', { name: 'New work item' })
  await dialog.getByLabel('Title').fill(title)
  await dialog.getByLabel('Type').selectOption({ label: 'Task' })
  await dialog.getByRole('button', { name: 'Create work item' }).click()

  // Creating opens the new item; its key is the Project key plus a number.
  await expect(page).toHaveURL(/\/work-items\/[^/]+$/)
  await expect(page.getByText(title).first()).toBeVisible()
  const key = (await page.getByText(new RegExp(`^${seeded.projectKey}-\\d+$`)).first().textContent())?.trim()
  expect(key).toMatch(new RegExp(`^${seeded.projectKey}-\\d+$`))

  await page.goto(`/projects/${seededValue('E2E_PROJECT_ID')}/work-items`)
  await page.getByRole('radiogroup', { name: 'View' }).getByRole('radio', { name: 'Board' }).click()
  await expect(page).toHaveURL(/view=board/)
  const board = page.getByRole('region', { name: 'Work item board' })
  const column = (name: string) => board.getByRole('region', { name: `${name} column` })
  const card = (name: string) => column(name).getByRole('listitem').filter({ hasText: title })

  // New items start in Backlog.
  await expect(card('Backlog')).toBeVisible()

  // Move with the keyboard-friendly status menu: Backlog → Ready.
  await card('Backlog').getByLabel(`Move ${key} to status`).selectOption({ label: 'Ready' })
  await expect(card('Ready')).toBeVisible()
  await expect(card('Backlog')).toHaveCount(0)

  // And by dragging the card: Ready → In Progress.
  await card('Ready').dragTo(column('In Progress'))
  await expect(card('In Progress')).toBeVisible()
  await expect(card('Ready')).toHaveCount(0)

  // The move is stored on the server, not just shown: it survives a reload.
  await page.reload()
  await expect(card('In Progress')).toBeVisible()
})
