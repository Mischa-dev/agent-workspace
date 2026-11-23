import { expect, test } from '@playwright/test'

import { seeded, storageStatePath } from '../environment'
import { downloadExport, importArchive, workspaceId, workspaces, writer } from '../support/done'

// Test for "done", item 9: export the whole workspace and import it on
// another server. The export is started and downloaded in the browser; the
// archive then goes in with the server's own `workspace-import` command, as
// the operator of another server would run it. Here it is this server, so
// it arrives as a copy with new ids.

test.use({ storageState: storageStatePath })

test('export the workspace from Settings, import the archive as a new workspace, and find its spaces there', async ({ page, context }, info) => {
  await page.goto('/w/-/settings/export')
  await page.getByRole('button', { name: 'Export the workspace' }).click()
  await expect(page.getByRole('link', { name: 'Download' }).first()).toBeVisible({ timeout: 60_000 })

  const { request, headers } = await writer(context, page)
  const archive = await downloadExport(request, await workspaceId(request), headers())
  const slug = `imported-${info.project.name}`
  importArchive(archive, seeded.loginName, slug)

  const imported = (await workspaces(page.request)).find((workspace) => workspace.slug === slug)
  expect(imported, 'the imported workspace is one of yours').toBeTruthy()
  const projects = (await (await page.request.get('/api/v1/projects?limit=100')).json()) as {
    projects: Array<{ name: string; organizationId: string }>
  }
  expect(projects.projects.filter((project) => project.organizationId === imported?.id).map((project) => project.name)).toContain(seeded.projectName)
  await page.goto(`/w/${slug}/home`)
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
  await expect(page.getByRole('main')).toContainText(seeded.projectName)
})
