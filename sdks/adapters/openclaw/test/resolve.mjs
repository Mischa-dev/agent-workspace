// Tests run without OpenClaw or an install: the runner SDK resolves to its
// source in this repository, and OpenClaw's plugin SDK to small stand-ins.
import { register } from 'node:module'

register('./hooks.mjs', import.meta.url)
