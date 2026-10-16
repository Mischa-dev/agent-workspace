import '@testing-library/jest-dom/vitest'
import { configure } from '@testing-library/react'

// The default one-second wait for findBy* and waitFor is too short when CI
// renders lazily loaded pages on a busy machine, which made tests flaky.
configure({ asyncUtilTimeout: 5000 })
