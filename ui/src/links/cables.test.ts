import { expect, test } from 'vitest'

import type { LinkView, TableView } from '../api/types'
import { cables, joinNames, mbps, publicText, rtt } from './cables'

let nextId = 1

function link(name: string, address: string, fields: Partial<LinkView> = {}): LinkView {
  return {
    id: nextId++,
    name,
    address,
    family: address.includes(':') ? 'ipv6' : 'ipv4',
    state: 'up',
    rtt_ms: 40,
    down_bps: null,
    public_ip: null,
    metered: false,
    ...fields,
  }
}

function table(links: LinkView[], shared: number[][] = [], connections: string[][] = []): TableView {
  return { links, shared, connections, combined: { bps: 0, parts: [], not_counted: [] } }
}

test('links on one cable make one card, in the order they appeared', () => {
  const t = table([
    link('Wi-Fi', '192.0.2.10'),
    link('Ethernet 2', '198.51.100.20'),
    link('Wi-Fi', '2001:db8::10'),
    link('Ethernet 3', '198.51.100.30'),
  ])

  const result = cables(t)

  expect(result.map((c) => c.name)).toEqual(['Wi-Fi', 'Ethernet 2', 'Ethernet 3'])
  expect(result[0].links.map((l) => l.address)).toEqual(['192.0.2.10', '2001:db8::10'])
})

test('no ranking by type: a phone that appeared first is first', () => {
  const t = table([link('Ethernet 2', '198.51.100.20'), link('Wi-Fi', '192.0.2.10')])
  expect(cables(t).map((c) => c.name)).toEqual(['Ethernet 2', 'Wi-Fi'])
})

test('no links, no cables', () => {
  expect(cables(table([]))).toEqual([])
})

test('the speed is shown once per cable, from a working address', () => {
  const t = table([
    // A dead address can still hold the figure from before it died.
    link('Wi-Fi', '192.0.2.10', { state: 'down', down_bps: 99_000_000 }),
    link('Wi-Fi', '2001:db8::10', { state: 'up', down_bps: 12_100_000 }),
    link('Ethernet 2', '198.51.100.20', { state: 'down', down_bps: 50_000_000 }),
    link('Ethernet 3', '198.51.100.30', { state: 'up', down_bps: null }),
    // The first working address with a figure, as render.rs's find_map.
    link('Ethernet 4', '198.51.100.40', { state: 'up', down_bps: null }),
    link('Ethernet 4', '2001:db8::40', { state: 'up', down_bps: 30_000_000 }),
    // Degraded still carries data, so its figure is real; a portal's is not.
    link('Ethernet 5', '198.51.100.50', { state: 'degraded', down_bps: 9_000_000 }),
    link('Ethernet 6', '198.51.100.60', { state: 'portal', down_bps: 88_000_000 }),
  ])

  expect(cables(t).map((c) => c.downBps)).toEqual([12_100_000, null, null, 30_000_000, 9_000_000, null])
})

test("the cable's state is its best address's, with that address's rtt", () => {
  const t = table([
    link('Wi-Fi', '192.0.2.10', { state: 'down', rtt_ms: null }),
    link('Wi-Fi', '2001:db8::10', { state: 'up', rtt_ms: 52 }),
    link('Ethernet 2', '198.51.100.20', { state: 'probing', rtt_ms: null }),
    link('Ethernet 2', '2001:db8::20', { state: 'portal', rtt_ms: 70 }),
    link('Ethernet 3', '198.51.100.30', { state: 'portal', rtt_ms: 80 }),
    link('Ethernet 3', '2001:db8::30', { state: 'degraded', rtt_ms: 900 }),
    link('Ethernet 4', '198.51.100.40', { state: 'down', rtt_ms: null }),
    link('Ethernet 4', '2001:db8::40', { state: 'probing', rtt_ms: null }),
    link('Ethernet 5', '198.51.100.50', { state: 'down', rtt_ms: null }),
    // Two equally good: the first one's.
    link('Ethernet 6', '198.51.100.60', { state: 'up', rtt_ms: 30 }),
    link('Ethernet 6', '2001:db8::60', { state: 'up', rtt_ms: 60 }),
  ])

  expect(cables(t).map((c) => [c.name, c.state, c.rttMs])).toEqual([
    ['Wi-Fi', 'up', 52],
    ['Ethernet 2', 'portal', 70],
    ['Ethernet 3', 'degraded', 900],
    ['Ethernet 4', 'probing', null],
    ['Ethernet 5', 'down', null],
    ['Ethernet 6', 'up', 30],
  ])
})

test("a cable names every other cable on its connection, as the engine joined them", () => {
  const wifi6 = link('Wi-Fi', '2001:db8::10', { public_ip: '2001:db8::1' })
  const eth2 = link('Ethernet 2', '198.51.100.20', { public_ip: '203.0.113.7' })
  const eth2v6 = link('Ethernet 2', '2001:db8::20', { public_ip: '2001:db8::1' })
  const eth3 = link('Ethernet 3', '198.51.100.30', { public_ip: '203.0.113.7' })
  const eth4 = link('Ethernet 4', '198.51.100.40', { public_ip: '203.0.113.9' })
  const eth5 = link('Ethernet 5', '198.51.100.50', { public_ip: '203.0.113.5' })
  const eth6 = link('Ethernet 6', '198.51.100.60', { public_ip: '203.0.113.5' })
  // Wi-Fi and Ethernet 3 share no address, but each shares one with
  // Ethernet 2, so all three are one connection, as the combined speed counts them.
  const t = table(
    [wifi6, eth2, eth2v6, eth3, eth4, eth5, eth6],
    [
      [eth5.id, eth6.id],
      [eth2.id, eth3.id],
      [wifi6.id, eth2v6.id],
    ],
    [
      ['Wi-Fi', 'Ethernet 2', 'Ethernet 3'],
      ['Ethernet 5', 'Ethernet 6'],
    ],
  )

  expect(cables(t).map((c) => c.sharedWith)).toEqual([
    ['Ethernet 2', 'Ethernet 3'],
    ['Wi-Fi', 'Ethernet 3'],
    ['Wi-Fi', 'Ethernet 2'],
    [],
    ['Ethernet 6'],
    ['Ethernet 5'],
  ])
})

test('formatting matches brg-links: 57600000 -> "57.6 Mbps", 40 -> "40 ms"', () => {
  expect(mbps(57_600_000)).toBe('57.6 Mbps')
  expect(mbps(56_537_504)).toBe('56.5 Mbps')
  expect(mbps(0)).toBe('0.0 Mbps')
  expect(mbps(999_999_999)).toBe('1000.0 Mbps')
  expect(rtt(40)).toBe('40 ms')
  expect(rtt(0)).toBe('0 ms')
})

test('an exact half rounds to the even digit, as Rust does', () => {
  // Checked against rustc's own `{:.1}`: JavaScript's toFixed rounds these up.
  expect(mbps(250_000)).toBe('0.2 Mbps')
  expect(mbps(750_000)).toBe('0.8 Mbps')
  expect(mbps(1_250_000)).toBe('1.2 Mbps')
  expect(mbps(1_750_000)).toBe('1.8 Mbps')
  // Not exact halves in binary, so not ties: these round as the nearest double does.
  expect(mbps(50_000)).toBe('0.1 Mbps')
  expect(mbps(150_000)).toBe('0.1 Mbps')
})

test('unknown figures show as "-", never 0', () => {
  expect(rtt(null)).toBe('-')
  expect(publicText(link('Wi-Fi', '192.0.2.10', { public_ip: null }))).toBe('-')
})

test('an IPv6 link that the internet sees as itself says "same as address"', () => {
  expect(publicText(link('Wi-Fi', '2001:db8::10', { public_ip: '2001:db8::10' }))).toBe('same as address')
  expect(publicText(link('Wi-Fi', '192.0.2.10', { public_ip: '203.0.113.7' }))).toBe('203.0.113.7')
})

test('names are joined as brg-links joins them', () => {
  expect(joinNames([])).toBe('')
  expect(joinNames(['Wi-Fi'])).toBe('Wi-Fi')
  expect(joinNames(['Wi-Fi', 'Ethernet 2'])).toBe('Wi-Fi and Ethernet 2')
  expect(joinNames(['Wi-Fi', 'Ethernet 2', 'Ethernet 3'])).toBe('Wi-Fi, Ethernet 2 and Ethernet 3')
})

test("a cable is metered when Windows says any of its addresses' connection is", () => {
  const t = table([
    link('Wi-Fi', '192.0.2.10'),
    link('Ethernet 3', '198.51.100.30'),
    link('Ethernet 3', '2001:db8::30', { metered: true }),
  ])
  expect(cables(t).map((c) => [c.name, c.metered])).toEqual([
    ['Wi-Fi', false],
    ['Ethernet 3', true],
  ])
})

test('each cable carries the data BARRAGE has used through it, 0 when none', () => {
  const t = table([link('Wi-Fi', '192.0.2.10'), link('Ethernet 2', '198.51.100.20'), link('Ethernet 3', '198.51.100.30')])
  const used = [
    { cable: 'Ethernet 2', bytes: 1_858_200_000 },
    { cable: 'Wi-Fi', bytes: 1_326_100_000 },
    // A cable that carried and has since gone has no card.
    { cable: 'Ethernet 9', bytes: 5 },
  ]
  expect(cables(t, used).map((c) => [c.name, c.used])).toEqual([
    ['Wi-Fi', 1_326_100_000],
    ['Ethernet 2', 1_858_200_000],
    ['Ethernet 3', 0],
  ])
  expect(cables(t).map((c) => c.used)).toEqual([0, 0, 0])
})
