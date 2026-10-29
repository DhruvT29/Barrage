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
 * once every byte is there. Worked out on whole numbers first, since
 * `29 / 100 * 100` is 28.999999999999996. An empty file is all there.
 */
export function percent(done: number, length: number): number {
  if (length === 0) return 100
  return Math.floor((done * 100) / length)
}
