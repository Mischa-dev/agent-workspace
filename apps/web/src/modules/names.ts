import names from 'virtual:modules/names'

/** Every core module, by id, in manifest order. */
export const MODULE_IDS: readonly string[] = Object.keys(names)

/** A module's name, such as "Tasks". */
export function moduleName(module: string) {
  return names[module] ?? module
}
