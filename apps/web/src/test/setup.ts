import '@testing-library/jest-dom/vitest'
import { configure } from '@testing-library/react'
import { afterAll, afterEach } from 'vitest'

import { touchTableLoading } from '../realtime/handlers/load'

// Routes load lazily, and vitest transforms a route's modules the first time a
// test opens it. On a busy CI machine that can take longer than the default
// one second, so findBy* and waitFor wait up to five.
configure({ asyncUtilTimeout: 5000 })

// Drafts are kept in localStorage; each test starts without the last one's.
afterEach(() => {
  window.localStorage.clear()
})

// A page that connected starts loading the realtime handler table; a file
// that ends first must not tear its environment down under that load.
afterAll(async () => {
  await touchTableLoading()?.catch(() => undefined)
})
