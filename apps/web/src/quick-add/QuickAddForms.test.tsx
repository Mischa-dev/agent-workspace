import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, describe, expect, it } from 'vitest'

import type { Project } from '../projects/types'
import { QuickTaskForm } from './QuickAddForms'

const projects = [
  { id: '019fb600-0000-7000-8000-00000000000a', key: 'TRI', name: 'Trident', archivedAt: null },
  { id: '019fb600-0000-7000-8000-00000000000b', key: 'NEP', name: 'Neptune', archivedAt: null },
] as unknown as Project[]

afterEach(() => {
  cleanup()
  window.localStorage.clear()
})

describe('QuickTaskForm', () => {
  it('keeps what was typed when the project changes', () => {
    render(
      <QueryClientProvider client={new QueryClient()}>
        <MemoryRouter>
          <QuickTaskForm projects={projects} shared={null} />
        </MemoryRouter>
      </QueryClientProvider>,
    )

    fireEvent.change(screen.getByLabelText('Title'), { target: { value: 'Fix the login page' } })
    fireEvent.change(screen.getByLabelText('Description'), { target: { value: 'It loops on Safari.' } })
    fireEvent.change(screen.getByLabelText('Project'), { target: { value: projects[1].id } })

    expect(screen.getByLabelText('Project')).toHaveValue(projects[1].id)
    expect(screen.getByLabelText('Title')).toHaveValue('Fix the login page')
    expect(screen.getByLabelText('Description')).toHaveValue('It loops on Safari.')
  })
})
