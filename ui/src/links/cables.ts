// The link table as the cards show it: one card per cable, and the figures
// written exactly as `brg-links` writes them (engine/src/links/render.rs).
import type { LinkState, LinkView, TableView, UseView } from '../api/types'

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
