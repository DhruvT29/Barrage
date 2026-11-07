// The transfer queue's arithmetic, apart from its markup: which filter a
// download falls under, its time left, and how its piece bar is coloured.
import type { DownloadState, DownloadView, QueueView, TorrentPhase, TorrentView } from '../api/types'
import { formatEta } from './format'

/** What the queue can be narrowed to. Every state falls under exactly one, besides All. */
export type Filter = 'all' | 'active' | 'torrents' | 'stopped' | 'done' | 'failed'

export const FILTERS: Filter[] = ['all', 'active', 'torrents', 'stopped', 'done', 'failed']

/** Active is everything still under way: finding the file, waiting for a cable, or fetching. */
export function filterOf(state: DownloadState): Exclude<Filter, 'all' | 'torrents'> {
  if (state === 'queued' || state === 'starting' || state === 'waiting' || state === 'downloading') return 'active'
  return state
}

/** A torrent falls under the same filters, and Torrents as well. */
export function torrentFilterOf(phase: TorrentPhase): Exclude<Filter, 'all' | 'torrents'> {
  return phase === 'queued' || phase === 'finding' || phase === 'downloading' ? 'active' : phase
}

/** How many downloads and torrents each filter would show. */
export function countByFilter(
  list: readonly DownloadView[],
  torrents: readonly TorrentView[] = [],
): Record<Filter, number> {
  const counts: Record<Filter, number> = {
    all: list.length + torrents.length,
    active: 0,
    torrents: torrents.length,
    stopped: 0,
    done: 0,
    failed: 0,
  }
  for (const d of list) counts[filterOf(d.state)] += 1
  for (const t of torrents) counts[torrentFilterOf(t.phase)] += 1
  return counts
}

/** What the time-left figure says in each state that is not fetching. */
const ETA_WORDS: Record<Exclude<DownloadState, 'downloading'>, string> = {
  queued: 'Queued',
  starting: 'Waiting',
  waiting: 'Waiting',
  stopped: 'Stopped',
  done: 'Complete',
  failed: 'Failed',
}

/** "3rd in line". */
export function inLine(place: number): string {
  const tens = place % 100
  const units = place % 10
  const teen = tens >= 11 && tens <= 13
  const suffix = teen ? 'th' : units === 1 ? 'st' : units === 2 ? 'nd' : units === 3 ? 'rd' : 'th'
  return `${place}${suffix} in line`
}

/** A transfer's place in line, by kind and id; undefined if it has none. */
export function placeOf(queue: QueueView | null | undefined, kind: 'web' | 'torrent', id: number): number | undefined {
  return queue?.places.find((p) => p.kind === kind && p.id === id)?.place
}

/**
 * The time left: the bytes left at the speed right now, while downloading;
 * "-" without a speed or a length to work it from; its place while it waits
 * in line; otherwise the state, in a word.
 */
export function etaText(d: DownloadView, place?: number): string {
  if (d.state === 'queued' && place !== undefined) return inLine(place)
  if (d.state !== 'downloading') return ETA_WORDS[d.state]
  if (d.length === null || d.bps <= 0) return '-'
  return formatEta((Math.max(0, d.length - d.done) * 8) / d.bps)
}

/** What the time-left figure says of a torrent that is not fetching. */
const TORRENT_ETA_WORDS: Record<Exclude<TorrentPhase, 'downloading'>, string> = {
  queued: 'Queued',
  finding: 'Finding',
  stopped: 'Stopped',
  done: 'Complete',
  failed: 'Failed',
}

/** A torrent's time left, as `etaText`. */
export function torrentEta(t: TorrentView, place?: number): string {
  if (t.phase === 'queued' && place !== undefined) return inLine(place)
  if (t.phase !== 'downloading') return TORRENT_ETA_WORDS[t.phase]
  if (t.length === null || t.down_bps <= 0) return '-'
  return formatEta((Math.max(0, t.length - t.done) * 8) / t.down_bps)
}

/** What the piece bar needs of a download or a torrent. */
export type Bar = Pick<DownloadView, 'state' | 'length' | 'done'> & { cables: { name: string; bytes: number }[] }

/** A torrent as the piece bar draws it: split by what each cable received. */
export function torrentBar(t: TorrentView): Bar {
  return {
    state: t.phase === 'finding' ? 'starting' : t.phase,
    length: t.length,
    done: t.done,
    cables: t.cables.map((c) => ({ name: c.name, bytes: c.received })),
  }
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
  d: Bar,
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

// The user's choice to open or close a card's list of cables, by download's
// id or `t` and a torrent's, kept while the page is open: a card drawn anew (filtered away and back, or
// the tab changed) opens as it was left.
const choices = new Map<string | number, boolean>()

export function openChoice(id: string | number): boolean | undefined {
  return choices.get(id)
}

export function rememberOpen(id: string | number, open: boolean): void {
  choices.set(id, open)
}

/** For tests: forget every choice, as a reload does. */
export function forgetChoices(): void {
  choices.clear()
}
