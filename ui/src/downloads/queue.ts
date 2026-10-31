// The transfer queue's arithmetic, apart from its markup: which filter a
// download falls under, its time left, and how its piece bar is coloured.
import type { DownloadState, DownloadView } from '../api/types'
import { formatEta } from './format'

/** What the queue can be narrowed to. Every state falls under exactly one, besides All. */
export type Filter = 'all' | 'active' | 'stopped' | 'done' | 'failed'

export const FILTERS: Filter[] = ['all', 'active', 'stopped', 'done', 'failed']

/** Active is everything still under way: finding the file, waiting for a cable, or fetching. */
export function filterOf(state: DownloadState): Exclude<Filter, 'all'> {
  if (state === 'starting' || state === 'waiting' || state === 'downloading') return 'active'
  return state
}

/** How many downloads each filter would show. */
export function countByFilter(list: readonly DownloadView[]): Record<Filter, number> {
  const counts: Record<Filter, number> = { all: list.length, active: 0, stopped: 0, done: 0, failed: 0 }
  for (const d of list) counts[filterOf(d.state)] += 1
  return counts
}

/** What the time-left figure says in each state that is not fetching. */
const ETA_WORDS: Record<Exclude<DownloadState, 'downloading'>, string> = {
  starting: 'Waiting',
  waiting: 'Waiting',
  stopped: 'Stopped',
  done: 'Complete',
  failed: 'Failed',
}

/**
 * The time left: the bytes left at the speed right now, while downloading;
 * "-" without a speed or a length to work it from; otherwise the state, in a
 * word.
 */
export function etaText(d: DownloadView): string {
  if (d.state !== 'downloading') return ETA_WORDS[d.state]
  if (d.length === null || d.bps <= 0) return '-'
  return formatEta((Math.max(0, d.length - d.done) * 8) / d.bps)
}

/** The piece bar's blocks: 10 px squares, 3 px apart. */
const BLOCK = 10
const GAP = 3

/** As many blocks as fit `width` pixels, between 16 and 80. */
export function blockCount(width: number): number {
  return Math.min(80, Math.max(16, Math.floor((width + GAP) / (BLOCK + GAP))))
}

/**
 * The piece bar, block by block: the colour of a filled block, or null for
 * one still to come. Blocks fill up to the part done, rounded down, so only
 * a finished download fills them all. The filled blocks are shared between
 * the cables in proportion to the bytes each carried, in the download's
 * order of cables, a run of each cable's colour; a spare block goes to the
 * largest remainder. A cable gone from the table is dim; blocks no cable
 * accounts for are green. Null when there is no length to fill against.
 */
export function pieceBlocks(
  d: DownloadView,
  colors: Record<string, string>,
  count: number,
): (string | null)[] | null {
  let filled: number
  if (d.state === 'done') filled = count
  else if (d.length === null) return null
  else filled = d.length === 0 ? count : Math.floor((Math.min(d.done, d.length) * count) / d.length)

  const carried = d.cables.filter((c) => c.bytes > 0)
  const total = carried.reduce((sum, c) => sum + c.bytes, 0)
  const blocks: (string | null)[] = []
  if (total === 0) {
    blocks.push(...Array<string>(filled).fill('var(--green)'))
  } else {
    const quotas = carried.map((c) => (filled * c.bytes) / total)
    const shares = quotas.map(Math.floor)
    let spare = filled - shares.reduce((a, b) => a + b, 0)
    // The largest remainders first; on a tie, the earlier cable.
    const order = quotas.map((_quota, i) => i).sort((a, b) => quotas[b] - shares[b] - (quotas[a] - shares[a]) || a - b)
    for (const i of order) {
      if (spare === 0) break
      shares[i] += 1
      spare -= 1
    }
    carried.forEach((c, i) => blocks.push(...Array<string>(shares[i]).fill(colors[c.name] ?? 'var(--dim)')))
  }
  while (blocks.length < count) blocks.push(null)
  return blocks
}

// The user's choice to open or close a card's list of cables, by download,
// kept while the page is open: a card drawn anew (filtered away and back, or
// the tab changed) opens as it was left.
const choices = new Map<number, boolean>()

export function openChoice(id: number): boolean | undefined {
  return choices.get(id)
}

export function rememberOpen(id: number, open: boolean): void {
  choices.set(id, open)
}

/** For tests: forget every choice, as a reload does. */
export function forgetChoices(): void {
  choices.clear()
}
