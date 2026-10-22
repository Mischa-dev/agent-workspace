// Captures the pictures the trailer's messages attach: node asset.mjs
import { mkdirSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { chromium } from '@playwright/test'

import { serve } from './serve.mjs'

const here = fileURLToPath(new URL('.', import.meta.url))
// The id the message attaches the picture by (data.ts, MUSE_MOCK).
const ASSETS = { 'export-mock': '7a1e0000-0000-4000-8000-00000000a001' }

mkdirSync(join(here, 'assets'), { recursive: true })
const server = await serve(5197)
const browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE })
const page = await browser.newPage({ viewport: { width: 1920, height: 1080 }, deviceScaleFactor: 1.5, locale: 'en-US', timezoneId: 'UTC' })
page.on('pageerror', (error) => console.error('page error:', error.message))
for (const [name, id] of Object.entries(ASSETS)) {
  await page.goto(`http://127.0.0.1:5197/?asset=${name}`)
  await page.waitForFunction(() => document.documentElement.dataset.ready === 'true')
  await page.locator('.tr-mock').screenshot({ path: join(here, 'assets', `${id}.png`) })
  console.log(name, id)
}
await browser.close()
server.close()
