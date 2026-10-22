// rough.js is Excalidraw's own copy (vite.config.ts), which the app doesn't
// depend on directly, so the few calls the sketches make are typed here.
declare module 'roughjs' {
  type Point = [number, number]
  type Options = Record<string, unknown>
  interface Drawable {
    shape: string
  }
  interface PathInfo {
    d: string
    stroke: string
    strokeWidth: number
    fill?: string
  }
  interface RoughGenerator {
    rectangle(x: number, y: number, width: number, height: number, options?: Options): Drawable
    polygon(points: Point[], options?: Options): Drawable
    curve(points: Point[], options?: Options): Drawable
    line(x1: number, y1: number, x2: number, y2: number, options?: Options): Drawable
    toPaths(drawable: Drawable): PathInfo[]
  }
  const rough: { generator(): RoughGenerator }
  export default rough
}
