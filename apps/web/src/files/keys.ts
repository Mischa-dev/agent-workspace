// Files' query keys, apart from their API.

export const fileKeys = {
  all: ['file'] as const,
  /** A file read on its own, such as one shown in a page. */
  file: (id: string) => ['file', id] as const,
}
