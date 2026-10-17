/** "    at ThreadPage (http://…)" → "ThreadPage": the component that threw, without URLs or props. */
export function failingComponent(componentStack: string | null | undefined) {
  const line = componentStack?.split('\n').find((entry) => entry.trim())
  const name = line?.trim().replace(/^at\s+/, '').split(/[\s@(]/)[0]
  return name || undefined
}
