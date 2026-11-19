const runner = new URL('../../../runner-typescript/src/index.ts', import.meta.url).href
const stub = new URL('./stubs/openclaw.ts', import.meta.url).href

export async function resolve(specifier, context, next) {
  if (specifier === '@agent-workspace/runner') return { url: runner, shortCircuit: true }
  if (specifier.startsWith('openclaw/plugin-sdk/')) return { url: stub, shortCircuit: true }
  return next(specifier, context)
}
