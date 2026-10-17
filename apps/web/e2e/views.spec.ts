import { expect, test } from '@playwright/test'

import { screenshotDir, seeded, seededValue, storageStatePath } from './environment'

// Views and data (W11): the sidebar's pinned views, My work, the timeline,
// and a Markdown plan imported through the preview.
test.use({ storageState: storageStatePath })

test('opens Ready for agents from the sidebar, then My work and the timeline', async ({ page }) => {
  const projectId = seededValue('E2E_PROJECT_ID')
  await page.goto(`/projects/${projectId}`)
  const sidebar = page.getByRole('complementary', { name: `${seeded.projectName} navigation` })
  await sidebar.getByRole('link', { name: 'Ready for agents' }).click()
  await expect(page).toHaveURL(/savedView=/)
  await expect(page).toHaveURL(/ready=true/)
  await expect(page.getByRole('button', { name: /^Ready for agents/ })).toBeVisible()

  await page.getByRole('navigation', { name: 'Projects' }).getByRole('link', { name: 'My work' }).click()
  await expect(page.getByRole('heading', { name: 'My work', exact: true })).toBeVisible()
  await page.screenshot({ path: `${screenshotDir}/my-work.png`, fullPage: true })

  await page.goto(`/projects/${projectId}/timeline`)
  await expect(page.getByRole('heading', { name: 'Timeline', exact: true })).toBeVisible()
})

test('imports a Markdown plan after a preview', async ({ page }) => {
  const projectId = seededValue('E2E_PROJECT_ID')
  const stamp = Date.now()
  await page.goto(`/settings/projects/${projectId}/import-export`)
  await page.getByLabel('Plan').fill(`## Launch ${stamp}\n- [ ] Write the notes ${stamp}\n  - [ ] Check the links ${stamp}\n- [x] Pick a date ${stamp}`)
  await page.getByRole('button', { name: 'Preview import' }).click()
  await expect(page.getByText(`Write the notes ${stamp}`)).toBeVisible()
  await page.screenshot({ path: `${screenshotDir}/import-preview.png`, fullPage: true })
  await page.getByRole('button', { name: 'Import 3 tasks' }).click()
  await expect(page.getByText('3 tasks created')).toBeVisible({ timeout: 30_000 })
})

test('shows what slips when a milestone moves earlier', async ({ page, context }) => {
  const projectId = seededValue('E2E_PROJECT_ID')
  const stamp = Date.now()
  const day = (offset: number) => {
    const date = new Date()
    date.setDate(date.getDate() + offset)
    return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
  }
  await page.goto(`/projects/${projectId}/milestones`)
  const csrf = (await context.cookies()).find((cookie) => cookie.name.endsWith('agent_workspace_csrf'))?.value ?? ''
  const created = await page.request.post(`/api/v1/projects/${projectId}/milestones`, {
    headers: { 'x-csrf-token': csrf, 'idempotency-key': `milestone-${stamp}`, origin: new URL(page.url()).origin },
    data: { name: `Beta ${stamp}`, targetDate: day(20) },
  })
  expect(created.ok(), await created.text()).toBe(true)
  const milestone = (await created.json()) as { id: string }

  // Two dated tasks in the milestone, brought in as a CSV.
  await page.goto(`/settings/projects/${projectId}/import-export`)
  await page.getByRole('radio', { name: 'CSV' }).click()
  await page.getByLabel('CSV file').setInputFiles({
    name: 'plan.csv',
    mimeType: 'text/csv',
    buffer: Buffer.from(
      `Title,Milestone,Start date,Due date\nDraft ${stamp},Beta ${stamp},${day(2)},${day(10)}\nPolish ${stamp},Beta ${stamp},${day(12)},${day(18)}\n`,
    ),
  })
  await page.getByRole('button', { name: 'Preview import' }).click()
  await page.getByRole('button', { name: 'Import 2 tasks' }).click()
  await expect(page.getByText('2 tasks created')).toBeVisible({ timeout: 30_000 })

  await page.goto(`/projects/${projectId}/timeline?milestoneId=${milestone.id}`)
  const diamond = page.getByRole('button', { name: new RegExp(`^Beta ${stamp}, target`) })
  await diamond.focus()
  for (let step = 0; step < 5; step += 1) await page.keyboard.press('ArrowLeft')
  await expect(page.getByRole('status').filter({ hasText: '1 task slips' })).toBeVisible()
  await page.screenshot({ path: `${screenshotDir}/timeline-slip.png`, fullPage: true })
  await page.keyboard.press('Escape')
  await expect(page.getByRole('status').filter({ hasText: 'slips' })).toHaveCount(0)
})
