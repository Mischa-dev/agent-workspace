// Renders the trailer to video: node render.mjs out.mp4 [fps] [workers] [from] [to]
// Each worker is its own browser drawing a run of frames, piped straight into
// its own ffmpeg; the runs are joined at the end without re-encoding.
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'

import { chromium } from '@playwright/test'

import { serve } from './serve.mjs'

const [out, fpsArg = '60', workersArg = '4', fromArg, toArg] = process.argv.slice(2)
if (!out) throw new Error('usage: node render.mjs out.mp4 [fps] [workers] [from] [to]')
const fps = Number(fpsArg)
const workers = Number(workersArg)
const PORT = 5300
const url = `http://127.0.0.1:${PORT}/`
const page = { viewport: { width: 1920, height: 1080 }, deviceScaleFactor: 1, locale: 'en-US', timezoneId: 'UTC' }
const work = join(dirname(resolve(out)), `.render-${process.pid}`)
mkdirSync(work, { recursive: true })

const server = await serve(PORT)

async function open() {
  const browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE })
  const tab = await browser.newPage(page)
  tab.on('pageerror', (error) => console.error('page error:', error.message))
  await tab.goto(url)
  await tab.waitForFunction(() => document.documentElement.dataset.ready === 'true')
  return { browser, tab }
}

const first = await open()
const duration = await first.tab.evaluate(() => window.__trailer.duration)
const from = Math.round(Number(fromArg ?? 0) * fps)
const to = Math.round(Number(toArg ?? duration) * fps)
const per = Math.ceil((to - from) / workers)
const started = performance.now()
let done = 0

async function segment(index, session) {
  const a = from + index * per
  const b = Math.min(to, a + per)
  if (a >= b) return null
  const file = join(work, `part-${index}.mp4`)
  const ffmpeg = spawn(
    'ffmpeg',
    ['-y', '-loglevel', 'error', '-f', 'image2pipe', '-framerate', String(fps), '-c:v', 'png', '-i', '-', '-c:v', 'libx264', '-preset', 'slow', '-crf', '14', '-tune', 'animation', '-pix_fmt', 'yuv420p', '-color_primaries', 'bt709', '-color_trc', 'bt709', '-colorspace', 'bt709', '-r', String(fps), file],
    { stdio: ['pipe', 'inherit', 'inherit'] },
  )
  const { browser, tab } = session ?? (await open())
  for (let frame = a; frame < b; frame += 1) {
    await tab.evaluate((seconds) => window.__trailer.seek(seconds), frame / fps)
    await tab.evaluate(() => Promise.all([...document.images].map((image) => image.decode().catch(() => undefined))))
    const png = await tab.screenshot({ type: 'png' })
    if (!ffmpeg.stdin.write(png)) await once(ffmpeg.stdin, 'drain')
    done += 1
    if (done % (fps * 2) === 0) {
      const rate = done / ((performance.now() - started) / 1000)
      console.log(`${done}/${to - from} frames, ${rate.toFixed(1)} a second, about ${Math.round((to - from - done) / rate)}s left`)
    }
  }
  ffmpeg.stdin.end()
  const [code] = await once(ffmpeg, 'close')
  await browser.close()
  if (code !== 0) throw new Error(`ffmpeg failed on part ${index}`)
  return file
}

const parts = (await Promise.all(Array.from({ length: workers }, (_, index) => segment(index, index === 0 ? first : undefined)))).filter(Boolean)
server.close()

const list = join(work, 'parts.txt')
writeFileSync(list, parts.map((file) => `file '${file}'`).join('\n'))
const join_ = spawn('ffmpeg', ['-y', '-loglevel', 'error', '-f', 'concat', '-safe', '0', '-i', list, '-c', 'copy', '-movflags', '+faststart', resolve(out)], { stdio: 'inherit' })
const [code] = await once(join_, 'close')
if (code !== 0) throw new Error('joining the parts failed')
rmSync(work, { recursive: true, force: true })
console.log(`wrote ${out}: ${to - from} frames in ${Math.round((performance.now() - started) / 1000)}s`)
