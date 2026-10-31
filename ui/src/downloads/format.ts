// Sizes as the page writes them: decimal units, as speeds are decimal Mbps,
// so a size and a speed read on the same scale.

const UNITS = ['kB', 'MB', 'GB', 'TB', 'PB']

/**
 * "999 B", "8.8 MB", "1.2 GB": whole bytes below 1 kB, then one decimal, in
 * powers of 1000. A size that rounds up to 1000 of one unit is written as
 * 1.0 of the next, never "1000.0 kB".
 */
export function formatBytes(bytes: number): string {
  if (bytes < 1000) return `${bytes} B`
  let value = bytes / 1000
  let unit = 0
  while (Number(value.toFixed(1)) >= 1000 && unit < UNITS.length - 1) {
    value /= 1000
    unit += 1
  }
  return `${value.toFixed(1)} ${UNITS[unit]}`
}

/**
 * How much of `length` is done, in whole percent, rounded down: 100% only
 * once every byte is there, and never more (it fills bars and progress
 * values). Worked out on whole numbers first, since `29 / 100 * 100` is
 * 28.999999999999996. An empty file is all there.
 */
export function percent(done: number, length: number): number {
  if (length === 0) return 100
  return Math.min(100, Math.floor((done * 100) / length))
}

/**
 * "48.5%": the percent a card shows, with one decimal, rounded down like
 * `percent`, so "100.0%" only once every byte is there. "-" while the server
 * has not said how long the file is: unknown, never 0.
 */
export function percentText(done: number, length: number | null): string {
  if (length === null) return '-'
  const tenths = length === 0 ? 1000 : Math.min(1000, Math.floor((done * 1000) / length))
  return `${(tenths / 10).toFixed(1)}%`
}

/**
 * A time left as its two largest units: "42s", "11m 52s", "1h 04m", "2d 5h".
 * Rounded up to the whole second, so a download a moment from done never
 * reads "0s" while it still has bytes to fetch.
 */
export function formatEta(seconds: number): string {
  const s = Math.ceil(seconds)
  const pad = (n: number) => String(n).padStart(2, '0')
  if (s < 60) return `${s}s`
  if (s < 3600) return `${Math.floor(s / 60)}m ${pad(s % 60)}s`
  if (s < 86_400) return `${Math.floor(s / 3600)}h ${pad(Math.floor((s % 3600) / 60))}m`
  return `${Math.floor(s / 86_400)}d ${Math.floor((s % 86_400) / 3600)}h`
}
