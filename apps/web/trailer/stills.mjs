// Renders single frames for checking: node stills.mjs out-dir 1.5 3 7.25 ...
import { mkdirSync } from 'node:fs'
import { join } from 'node:path'

import { chromium } from '@playwright/test'

import { serve } from './serve.mjs'

const [out, ...times] = process.argv.slice(2)
mkdirSync(out, { recursive: true })
const server = await serve(5198)
const browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE })
const page = await browser.newPage({ viewport: { width: 1920, height: 1080 }, deviceScaleFactor: Number(process.env.SCALE ?? 1), locale: 'en-US', timezoneId: 'UTC' })
page.on('console', (message) => { if (message.type() === 'error') console.error('page:', message.text()) })
page.on('pageerror', (error) => console.error('page error:', error.message))
await page.goto('http://127.0.0.1:5198/')
await page.waitForFunction(() => document.documentElement.dataset.ready === 'true')
for (const time of times) {
  const started = performance.now()
  await page.evaluate((seconds) => window.__trailer.seek(seconds), Number(time))
  await page.evaluate(() => Promise.all([...document.images].map((image) => image.decode().catch(() => undefined))))
  await page.screenshot({ path: join(out, `t${Number(time).toFixed(2).padStart(6, '0')}.png`) })
  console.log(time, `${Math.round(performance.now() - started)}ms`)
}
await browser.close()
server.close()
