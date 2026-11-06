// The module manifests' build-time projections (moduleProjections in
// vite.config.ts): each kind's [pattern, module] claims, the modules that can
// be off, and each module's name.
declare module 'virtual:modules/slots/*' {
  const claims: ReadonlyArray<readonly [pattern: string, module: string]>
  export default claims
}
declare module 'virtual:modules/switchable' {
  const modules: readonly string[]
  export default modules
}
declare module 'virtual:modules/names' {
  const names: Readonly<Record<string, string>>
  export default names
}
