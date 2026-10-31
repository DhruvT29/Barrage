// The link table as the cards show it: one card per cable, and the figures
// written exactly as `brg-links` writes them (engine/src/links/render.rs).
import type { DownloadsView, LinkState, LinkView, TableView, UseView } from '../api/types'

/** One interface, with every address on it. */
export type Cable = {
  name: string
  /** Its addresses, in the order they appeared. */
  links: LinkView[]
  /** Its best address's state: the cable works if any address on it works. */
  state: LinkState
  /** That address's round trip, when it has one. */
  rttMs: number | null
  /** The measured speed, from a usable address; null until measured. */
  downBps: number | null
  /** The other cables that reach the internet through the same connection, directly or through another cable. */
  sharedWith: string[]
  /** Whether Windows says the connection any of its addresses is on is metered. */
  metered: boolean
  /** Bytes BARRAGE has downloaded through it since the engine started, written or not; 0 until it carries. */
  used: number
}

/** The states that carry data, as the engine's `LinkState::is_usable`. */
const USABLE: LinkState[] = ['up', 'degraded']

/** Best first. Only for a cable's own addresses, never to rank cables. */
const BEST: LinkState[] = ['up', 'degraded', 'portal', 'probing', 'down']

/**
 * The cables, in the order each first appeared. No ranking by type. `used`
 * is the downloads' data used by cable; a cable it does not name has used
 * nothing.
 */
export function cables(table: TableView, used: UseView[] = []): Cable[] {
  const names: string[] = []
  for (const link of table.links) {
    if (!names.includes(link.name)) names.push(link.name)
  }
  return names.map((name) => {
    const links = table.links.filter((l) => l.name === name)
    const best = links.reduce((a, b) => (BEST.indexOf(b.state) < BEST.indexOf(a.state) ? b : a))
    // As render.rs's cable_speed: the first usable address (up or degraded)
    // with a figure. A dead address, or a portal's, can still hold one from before.
    const speedFrom = links.find((l) => USABLE.includes(l.state) && l.down_bps !== null)

    // The engine joins cables into connections once, the same way the
    // combined speed counts them, so the card and the total cannot disagree.
    const connection = table.connections.find((cables) => cables.includes(name)) ?? []
    const sharedWith = connection.filter((other) => other !== name)

    return {
      name,
      links,
      state: best.state,
      rttMs: best.rtt_ms,
      downBps: speedFrom?.down_bps ?? null,
      sharedWith,
      metered: links.some((l) => l.metered),
      used: used.find((u) => u.cable === name)?.bytes ?? 0,
    }
  })
}

/**
 * "57.6 Mbps", as Rust's `{:.1}` writes `bps / 1e6`. Both round the same
 * double the same way, except an exact half, which Rust rounds to the even
 * digit and `toFixed` rounds up. An exact half happens only for an odd
 * multiple of 250 000 bits per second.
 */
export function mbps(bps: number): string {
  if (bps % 500_000 === 250_000) {
    const tenths = Math.floor(bps / 100_000)
    const even = tenths % 2 === 0 ? tenths : tenths + 1
    return `${(even / 10).toFixed(1)} Mbps`
  }
  return `${(bps / 1e6).toFixed(1)} Mbps`
}

/** "40 ms", or "-" before the first probe. */
export function rtt(ms: number | null): string {
  return ms === null ? '-' : `${ms} ms`
}

/** What the internet sees this address as. */
export function publicText(link: LinkView): string {
  if (link.public_ip === null) return '-'
  // The engine writes both addresses in the same canonical form.
  if (link.public_ip === link.address) return 'same as address'
  return link.public_ip
}

/** "A", "A and B", "A, B and C", as render.rs joins names. */
export function joinNames(names: string[]): string {
  if (names.length <= 1) return names.join('')
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`
}

/**
 * What the downloads fetch through this cable right now, in bits per second;
 * null while it carries nothing (no part, or a part at 0), so "carrying" is
 * a single test everywhere.
 */
export function liveBps(downloads: DownloadsView | null, name: string): number | null {
  const bps = downloads?.live.parts.find((p) => p.name === name)?.bps ?? 0
  return bps > 0 ? bps : null
}

/**
 * A cable's part of the live total, 0 to 1. Unknown (null) while nothing
 * downloads; a cable that carries nothing while others do has a real 0.
 */
export function liveShare(bps: number | null, total: number): number | null {
  if (total <= 0) return null
  return (bps ?? 0) / total
}

/** How much of its measured speed a cable is using: unknown unless both are known. */
export function utilization(live: number | null, measured: number | null): number | null {
  if (live === null || measured === null || measured <= 0) return null
  return live / measured
}

/**
 * A fraction as "61%", whole, or "-" when unknown. Can pass 100%: a
 * measurement can be beaten. (A download's own percent done, rounded down
 * to a tenth, is `percentText` in downloads/format.ts.)
 */
export function fractionText(fraction: number | null): string {
  return fraction === null ? '-' : `${Math.round(fraction * 100)}%`
}

/** The cable with the highest measured speed (the first on a tie), or null before any is measured. */
export function fastest(list: Cable[]): string | null {
  let best: Cable | null = null
  for (const cable of list) {
    if (cable.downBps !== null && cable.downBps > 0 && (best === null || cable.downBps > best.downBps!)) best = cable
  }
  return best?.name ?? null
}

/** "2 addresses · IPv4 + IPv6": how many addresses a cable has, and of which families. */
export function addressSummary(links: LinkView[]): string {
  const families = [
    ...(links.some((l) => l.family === 'ipv4') ? ['IPv4'] : []),
    ...(links.some((l) => l.family === 'ipv6') ? ['IPv6'] : []),
  ]
  const count = `${links.length} ${links.length === 1 ? 'address' : 'addresses'}`
  return families.length ? `${count} · ${families.join(' + ')}` : count
}

export type InterfaceKind = 'wifi' | 'bluetooth' | 'cable'

/**
 * Which icon a cable gets, from the only thing the engine says about it: the
 * interface's name, as Windows gives it. Anything not plainly wireless or
 * Bluetooth is drawn as a cable, a tethered phone included, since nothing
 * tells it apart from an adapter.
 */
export function interfaceKind(name: string): InterfaceKind {
  if (/wi-?fi|wlan|wireless/i.test(name)) return 'wifi'
  if (/bluetooth/i.test(name)) return 'bluetooth'
  return 'cable'
}
