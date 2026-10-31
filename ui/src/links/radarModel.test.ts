import { expect, test } from 'vitest'

import type { DownloadsView, LinkState } from '../api/types'
import type { Cable } from './cables'
import {
  bearing,
  filterCables,
  labelPlace,
  NODE_NAME_MAX,
  nodeName,
  nodeRadius,
  polar,
  reach,
  stateCounts,
  stateWords,
  tone,
} from './radarModel'

function cable(name: string, fields: Partial<Cable> = {}): Cable {
  return {
    name,
    links: [],
    state: 'up',
    rttMs: 20,
    downBps: 18_000_000,
    sharedWith: [],
    metered: false,
    used: 0,
    ...fields,
  }
}

test('the distance from the hub is the round trip, on a range of 100 ms', () => {
  expect(reach(0)).toBe(0)
  expect(reach(25)).toBe(0.25)
  expect(reach(100)).toBe(1)
  // Beyond the range sits on the edge.
  expect(reach(900)).toBe(1)
  // No round trip yet: on the edge too (drawn hollow).
  expect(reach(null)).toBe(1)
})

test('a node is placed between the hub and the edge by its reach', () => {
  expect(nodeRadius(0, 240, 40)).toBe(40)
  expect(nodeRadius(50, 240, 40)).toBe(140)
  expect(nodeRadius(100, 240, 40)).toBe(240)
  expect(nodeRadius(null, 240, 40)).toBe(240)
})

test('cables are spread evenly round the hub, in table order', () => {
  expect([0, 1, 2, 3].map((i) => bearing(i, 4))).toEqual([45, 135, 225, 315])
  expect(bearing(0, 1)).toBe(180)
  expect([0, 1, 2].map((i) => bearing(i, 3))).toEqual([60, 180, 300])
})

test('bearings are clockwise from the top', () => {
  const north = polar(300, 300, 100, 0)
  const east = polar(300, 300, 100, 90)
  const south = polar(300, 300, 100, 180)
  expect(north.x).toBeCloseTo(300)
  expect(north.y).toBeCloseTo(200)
  expect(east.x).toBeCloseTo(400)
  expect(east.y).toBeCloseTo(300)
  expect(south.y).toBeCloseTo(400)
})

test('a node is green when up, amber while degraded, probing or behind a portal, red when down', () => {
  const tones = (['up', 'degraded', 'probing', 'portal', 'down'] as LinkState[]).map(tone)
  expect(tones).toEqual(['up', 'warn', 'warn', 'warn', 'down'])
})

test('the state is written in words, with "carrying" while it carries', () => {
  expect(stateWords('up', true)).toBe('up · carrying')
  expect(stateWords('degraded', false)).toBe('degraded')
  expect(stateWords('down', false)).toBe('down')
})

const downloads = (parts: { name: string; bps: number }[]): DownloadsView => ({
  live: { bps: parts.reduce((a, p) => a + p.bps, 0), parts },
  list: [],
  used: [],
})

test('the filter shows every cable, only those carrying, or only the idle ones', () => {
  const all = [cable('Wi-Fi'), cable('Ethernet'), cable('Ethernet 2'), cable('Bluetooth Network', { state: 'down' })]
  const d = downloads([
    { name: 'Ethernet', bps: 22_000_000 },
    { name: 'Wi-Fi', bps: 11_000_000 },
    { name: 'Ethernet 2', bps: 0 },
  ])
  const names = (list: Cable[]) => list.map((c) => c.name)

  expect(names(filterCables(all, 'all', d))).toEqual(['Wi-Fi', 'Ethernet', 'Ethernet 2', 'Bluetooth Network'])
  // Table order, not the order of the parts.
  expect(names(filterCables(all, 'carrying', d))).toEqual(['Wi-Fi', 'Ethernet'])
  expect(names(filterCables(all, 'idle', d))).toEqual(['Ethernet 2', 'Bluetooth Network'])
  // No downloads at all: everything is idle.
  expect(names(filterCables(all, 'carrying', null))).toEqual([])
  expect(names(filterCables(all, 'idle', null))).toHaveLength(4)
})

test('the states table counts the cables in each state, every state listed', () => {
  const all = [
    cable('Wi-Fi'),
    cable('Ethernet'),
    cable('Ethernet 2', { state: 'degraded' }),
    cable('Bluetooth Network', { state: 'down' }),
  ]
  expect(stateCounts(all)).toEqual([
    ['up', 2],
    ['degraded', 1],
    ['probing', 0],
    ['portal', 0],
    ['down', 1],
  ])
})

test('labels sit away from the hub near the centre, toward it near the edge, above or below at the top and bottom', () => {
  // Right of the hub, near it: labels to the right (outward).
  expect(labelPlace(90, 0.2)).toMatchObject({ anchor: 'start', dx: 24 })
  // Right of the hub, at the edge: labels to the left (inward), off the bearing marks.
  expect(labelPlace(90, 1)).toMatchObject({ anchor: 'end', dx: -24 })
  expect(labelPlace(270, 0.2)).toMatchObject({ anchor: 'end' })
  expect(labelPlace(270, 1)).toMatchObject({ anchor: 'start' })
  // Straight down near the hub: below it; at the edge: above it.
  expect(labelPlace(180, 0.2)).toMatchObject({ anchor: 'middle', nameDy: 34 })
  expect(labelPlace(180, 1)).toMatchObject({ anchor: 'middle', nameDy: -36 })
  // Straight up near the hub: above it.
  expect(labelPlace(0, 0.2)).toMatchObject({ anchor: 'middle', nameDy: -36 })
})

test('a node label keeps a short name whole and cuts a long one to fit beside the node, marked as cut', () => {
  expect(nodeName('Wi-Fi')).toBe('Wi-Fi')
  expect(nodeName('Bluetooth Network')).toBe('Bluetooth Network')
  const long = 'Remote NDIS based Internet Sharing Device #2'
  const cut = nodeName(long)
  expect(cut.length).toBe(NODE_NAME_MAX)
  expect(cut.endsWith('…')).toBe(true)
  expect(long.startsWith(cut.slice(0, -1).trimEnd())).toBe(true)
})
