// Evaluates an expression at moments of the trailer, for checking layout: node probe.mjs 'expression' 1.5 3 ...
import { chromium } from '@playwright/test'

import { serve } from './serve.mjs'

const [expression, ...times] = process.argv.slice(2)
const server = await serve(5197)
const browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE })
const page = await browser.newPage({ viewport: { width: 1920, height: 1080 }, locale: 'en-US', timezoneId: 'UTC' })
page.on('pageerror', (error) => console.error('page error:', error.message))
await page.goto('http://127.0.0.1:5197/')
await page.waitForFunction(() => document.documentElement.dataset.ready === 'true')
for (const time of times) {
  await page.evaluate((seconds) => window.__trailer.seek(seconds), Number(time))
  const value = await page.evaluate(expression)
  console.log(time, JSON.stringify(value))
}
await browser.close()
server.close()
