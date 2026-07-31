import { describe, expect, it } from 'vitest'

import { normalizeProject } from './api'

function projectWithCapabilities(capabilities: unknown) {
  return normalizeProject({
    id: 'project-a',
    organizationId: 'organization-a',
    key: 'A',
    name: 'Project A',
    role: 'member',
    version: 1,
    capabilities,
    navigation: {
      location: { kind: 'project_overview' },
      version: 1,
    },
  })
}

describe('Project wire normalization', () => {
  it('fails every missing capability closed', () => {
    expect(projectWithCapabilities(undefined).capabilities).toEqual({
      view: false,
      manage: false,
      archive: false,
      navigationWrite: false,
    })
    expect(
      projectWithCapabilities({ manage: true }).capabilities,
    ).toEqual({
      view: false,
      manage: true,
      archive: false,
      navigationWrite: false,
    })
  })

  it('accepts only explicit boolean capability grants', () => {
    expect(
      projectWithCapabilities({
        view: 'true',
        navigationWrite: 1,
        archive: true,
      }).capabilities,
    ).toEqual({
      view: false,
      manage: false,
      archive: true,
      navigationWrite: false,
    })
  })
})
