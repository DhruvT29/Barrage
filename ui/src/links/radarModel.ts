// The radar's geometry and its small rules, kept pure so they are tested
// without drawing: where a cable sits (its bearing by table order, its
// distance by round trip), its colour by state, and the filter.
import type { DownloadsView, LinkState } from '../api/types'
import { type Cable, liveBps, mbps } from './cables'

/** The radar's range: a round trip of this many milliseconds reaches the edge. */
export const RANGE_MS = 100

/** The rings drawn on the radar, in milliseconds. */
export const RINGS_MS = [25, 50, 75, 100]

/**
 * How far out a cable sits, 0 at the hub to 1 at the edge: its round trip
 * over the range. Beyond the range, and before the first probe, it sits on
 * the edge (drawn hollow when there is no round trip yet).
 */
export function reach(rttMs: number | null): number {
  if (rttMs === null) return 1
  return Math.min(Math.max(rttMs, 0) / RANGE_MS, 1)
}

/** The node's distance from the centre, between the hub's edge `inner` and the radar's edge `outer`. */
export function nodeRadius(rttMs: number | null, outer: number, inner: number): number {
  return inner + reach(rttMs) * (outer - inner)
}

/**
 * The cable's bearing in degrees, clockwise from the top: evenly spread in
 * table order, half a step off the top so no node sits on the 000 mark.
 * The bearing means nothing more than its place in the table.
 */
export function bearing(index: number, count: number): number {
  return ((index + 0.5) * 360) / count
}

/** The point at `deg` (clockwise from the top) and `r` from (cx, cy). */
export function polar(cx: number, cy: number, r: number, deg: number): { x: number; y: number } {
  const rad = (deg * Math.PI) / 180
  return { x: cx + r * Math.sin(rad), y: cy - r * Math.cos(rad) }
}

/** The three colours a node can be: working, in doubt, not working. */
export type Tone = 'up' | 'warn' | 'down'

export function tone(state: LinkState): Tone {
  if (state === 'up') return 'up'
  if (state === 'down') return 'down'
  return 'warn'
}

/** The token for a tone's colour. */
export const TONE_COLOR: Record<Tone, string> = {
  up: 'var(--green)',
  warn: 'var(--amber)',
  down: 'var(--red)',
}

/** "up · carrying", or the engine's word alone while the cable carries nothing. */
export function stateWords(state: LinkState, carrying: boolean): string {
  return carrying ? `${state} · carrying` : state
}

export type Filter = 'all' | 'carrying' | 'idle'

export const FILTERS: { value: Filter; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'carrying', label: 'Carrying' },
  { value: 'idle', label: 'Idle' },
]

/** The cables the filter shows, in table order. */
export function filterCables(list: Cable[], filter: Filter, downloads: DownloadsView | null): Cable[] {
  if (filter === 'all') return list
  return list.filter((c) => (liveBps(downloads, c.name) !== null) === (filter === 'carrying'))
}

/** Every state the engine has, in the order the states table lists them. */
const STATES: LinkState[] = ['up', 'degraded', 'probing', 'portal', 'down']

/** How many cables are in each state, every state listed (a 0 is a real count). */
export function stateCounts(list: Cable[]): [LinkState, number][] {
  return STATES.map((state) => [state, list.filter((c) => c.state === state).length])
}

/**
 * A node's or a roster row's name for screen readers: the cable, its state
 * in words, its round trip and its measured speed, unknowns said in words.
 */
export function nodeLabel(cable: Cable, carrying: boolean): string {
  const state = carrying ? `${cable.state}, carrying` : cable.state
  const ping = cable.rttMs === null ? 'no round trip yet' : `${cable.rttMs} ms`
  const speed = cable.downBps === null ? 'not measured' : mbps(cable.downBps)
  return `${cable.name}: ${state}, ${ping}, ${speed}`
}

/** What to say when the filter leaves nothing to show. */
export const FILTER_EMPTY: Record<Filter, string> = {
  all: '',
  carrying: 'No cable is carrying a download right now.',
  idle: 'Every cable is carrying a download.',
}

/** Where a node's two label lines go, relative to the node. */
export type LabelPlace = { anchor: 'start' | 'middle' | 'end'; dx: number; nameDy: number; figDy: number }

/**
 * A node's labels sit beside it, away from the hub while the node is near
 * the hub (so they never cover "THIS PC") and toward it once the node is
 * near the edge (so they never run over the bearing marks). A node at the
 * very top or bottom keeps them above or below it.
 */
export function labelPlace(deg: number, nodeReach: number): LabelPlace {
  const rad = (deg * Math.PI) / 180
  const side = Math.sin(rad)
  const up = Math.cos(rad) > 0
  const outward = nodeReach < 0.7
  if (Math.abs(side) < 0.35) {
    // Top or bottom: above or below, on the side away from the hub (or toward it at the edge).
    const below = up !== outward
    return below
      ? { anchor: 'middle', dx: 0, nameDy: 34, figDy: 48 }
      : { anchor: 'middle', dx: 0, nameDy: -36, figDy: -22 }
  }
  const right = side > 0 === outward
  return { anchor: right ? 'start' : 'end', dx: right ? 24 : -24, nameDy: -3, figDy: 11 }
}

/** The most characters a node's name label holds: about what fits between a node and the radar's far edge. */
export const NODE_NAME_MAX = 22

/**
 * A node's name label: the cable's name, cut with "…" when it would run out
 * of the radar (Windows names some adapters at length). The node's own name
 * for screen readers, and its tooltip, keep the whole name.
 */
export function nodeName(name: string): string {
  return name.length <= NODE_NAME_MAX ? name : `${name.slice(0, NODE_NAME_MAX - 1).trimEnd()}…`
}
