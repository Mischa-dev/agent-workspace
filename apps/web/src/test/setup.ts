import '@testing-library/jest-dom/vitest'
import { configure } from '@testing-library/react'
import { afterEach } from 'vitest'

// Routes load lazily, and vitest transforms a route's modules the first time a
// test opens it. On a busy CI machine that can take longer than the default
// one second, so findBy* and waitFor wait up to five.
configure({ asyncUtilTimeout: 5000 })

// Drafts are kept in localStorage; each test starts without the last one's.
afterEach(() => {
  window.localStorage.clear()
})
