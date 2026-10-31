// The hero's and the header's figures, worked out from the engine's own
// fields and nothing else. Small pure functions, so each rule is tested on
// its own: unknown is null (drawn as "-"), never 0.
import type { CombinedView, DownloadView, DownloadsView, NotCounted, TableView } from '../api/types'
import { formatEta } from '../downloads/format'
import { cables, joinNames, mbps } from '../links/cables'

/** What the hero's big figure is: the live download speed, or the combined speed available. */
export type HeroFigure = {
  mode: 'live' | 'available'
  /** Bits per second; null when nothing is measured yet. */
  bps: number | null
  /** By cable while downloading, by independent connection otherwise. */
  parts: { name: string; bps: number }[]
  /** Downloads downloading. */
  running: number
  /** The combined speed available, in bits per second; null until measured. */
  available: number | null
}

/** The downloads downloading. Running is a state, not a speed: one that stalls keeps the hero on it. */
export function downloadingCount(downloads: DownloadsView | null): number {
  return downloads?.list.filter((d) => d.state === 'downloading').length ?? 0
}

/** The combined speed available, or null while nothing is measured: the engine's 0 then means "no parts". */
function availableBps(table: TableView | null): number | null {
  return table && table.combined.parts.length > 0 ? table.combined.bps : null
}

/**
 * While anything downloads, the live download speed, a part per cable
 * carrying data; otherwise the combined speed available, a part per
 * independent connection. The engine works out both (`api/view.rs`).
 */
export function heroFigure(table: TableView | null, downloads: DownloadsView | null): HeroFigure {
  const running = downloadingCount(downloads)
  const available = availableBps(table)
  if (downloads && running > 0) {
    return { mode: 'live', bps: downloads.live.bps, parts: downloads.live.parts, running, available }
  }
  const parts = (table?.combined.parts ?? []).map(({ name, bps }) => ({ name, bps }))
  return { mode: 'available', bps: available, parts, running, available }
}

/** "1 cable", "2 cables". */
export function count(n: number, thing: string): string {
  return `${n} ${thing}${n === 1 ? '' : 's'}`
}

/** The share of the available speed the downloads use, in whole percent; null when it has no meaning. */
export function usedShare(figure: HeroFigure): number | null {
  if (figure.mode !== 'live' || figure.bps === null || !figure.available) return null
  return Math.round((figure.bps / figure.available) * 100)
}

/** The line under the big figure: what it is out of, or where it comes from. */
export function gaugeCaption(figure: HeroFigure): string {
  if (figure.mode === 'live') {
    const share = usedShare(figure)
    if (share !== null && figure.available !== null && figure.parts.length > 0) {
      return `${share}% of ${mbps(figure.available)}`
    }
    const through = figure.parts.length === 0 ? 'nothing arriving right now' : `through ${count(figure.parts.length, 'cable')}`
    return `${count(figure.running, 'download')}, ${through}`
  }
  if (figure.parts.length === 0) return 'nothing measured yet'
  if (figure.parts.length === 1) return 'from 1 connection'
  return `from ${figure.parts.length} independent connections`
}

/** What the hero's note says the figure is, as the old hero said it. */
export const LIVE_NOTE =
  'What every download is fetching right now, added up by cable. With nothing downloading, this shows the combined speed available.'

/** What the combined speed counted, what it counted once (a shared connection) and what it left out. */
export function countedNote(combined: CombinedView): string {
  const notes: string[] = ["The sum of each working connection's measured speed."]
  for (const part of combined.parts) {
    const n = part.shared_with.length
    if (n > 0) {
      notes.push(
        `${joinNames(part.shared_with)} ${n === 1 ? 'shares' : 'share'} ${part.name}'s connection, ` +
          `so ${n === 1 ? 'it is' : 'they are'} counted once with it.`,
      )
    }
  }
  if (combined.not_counted.length > 0) {
    notes.push(`Not counted: ${combined.not_counted.map((c) => `${c.name} (${c.why})`).join(', ')}.`)
  }
  return notes.join(' ')
}

/** "1 cable not counted: Bluetooth Network (down)". */
export function notCountedText(left: NotCounted[]): string {
  return `${count(left.length, 'cable')} not counted: ${left.map((c) => `${c.name} (${c.why})`).join(', ')}`
}

/** Cables that carry data (up or degraded), each counted once; null before the first table. */
export function linkedCables(table: TableView | null): number | null {
  if (!table) return null
  return cables(table).filter((c) => c.state === 'up' || c.state === 'degraded').length
}

/** The mean round trip of the usable cables that have one, in whole ms; null when none has. */
export function averagePing(table: TableView | null): number | null {
  if (!table) return null
  const rtts = cables(table)
    .filter((c) => (c.state === 'up' || c.state === 'degraded') && c.rttMs !== null)
    .map((c) => c.rttMs as number)
  if (rtts.length === 0) return null
  return Math.round(rtts.reduce((a, b) => a + b, 0) / rtts.length)
}

/** Every byte the cables have received for the downloads since the engine started; null before the first downloads. */
export function fetched(downloads: DownloadsView | null): number | null {
  if (!downloads) return null
  return downloads.used.reduce((sum, u) => sum + u.bytes, 0)
}

/** The bytes a download still has to fetch; null while its size is unknown. */
function left(d: DownloadView): number | null {
  return d.length === null ? null : Math.max(0, d.length - d.done)
}

/** The downloading download with the most left to fetch; one of unknown size only when no size is known. */
export function leadTransfer(downloads: DownloadsView | null): DownloadView | null {
  const running = downloads?.list.filter((d) => d.state === 'downloading') ?? []
  if (running.length === 0) return null
  const sized = running.filter((d) => d.length !== null)
  if (sized.length === 0) return running[0]
  return sized.reduce((a, b) => ((left(b) ?? 0) > (left(a) ?? 0) ? b : a))
}

/**
 * "8s", "11m 52s", "1h 06m", "1d 2h": the time left at the download's own
 * speed, written as its card writes it (format.ts); "-" when it cannot be known.
 */
export function eta(d: DownloadView): string {
  const bytes = left(d)
  if (bytes === null || d.bps <= 0) return '-'
  return formatEta((bytes * 8) / d.bps)
}

/** "00:03:36": hours grow past 99 rather than wrap. */
export function clock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000))
  const two = (n: number) => String(n).padStart(2, '0')
  return `${two(Math.floor(total / 3600))}:${two(Math.floor((total % 3600) / 60))}:${two(total % 60)}`
}

/** The downloads Stop all asks: the ones a card's Stop would (starting, waiting, downloading). */
export function stoppable(downloads: DownloadsView | null): DownloadView[] {
  return downloads?.list.filter((d) => d.state === 'starting' || d.state === 'waiting' || d.state === 'downloading') ?? []
}

/** "57.6": a speed's figure without its unit, for chips and tiles. */
export function mbpsFigure(bps: number): string {
  return mbps(bps).split(' ')[0]
}

/**
 * What Stop all says of its refusals, each wording once: with the downloads
 * it was for, or with how many when every download asked got it (the engine
 * not answering, say), rather than the same words once per download.
 */
export function refusalLines(refused: { name: string; words: string }[], asked: number): string[] {
  const byWords = new Map<string, string[]>()
  for (const { name, words } of refused) byWords.set(words, [...(byWords.get(words) ?? []), name])
  return [...byWords].map(([words, names]) =>
    asked > 1 && names.length === asked ? `${count(asked, 'download')}: ${words}` : `${names.join(', ')}: ${words}`,
  )
}
