import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { App } from './App'

describe('application shell', () => {
  it('renders the four-region M0 shell', () => {
    render(<App />)

    expect(screen.getByRole('navigation', { name: 'Projects' })).toBeVisible()
    expect(screen.getByRole('heading', { name: 'Agent Workspace' })).toBeVisible()
    expect(screen.getByRole('heading', { name: '# general' })).toBeVisible()
    expect(screen.getByRole('heading', { name: 'Project host' })).toBeVisible()
  })
})
