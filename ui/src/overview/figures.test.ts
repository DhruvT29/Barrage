import { expect, test } from 'vitest'

import { etaText } from '../downloads/queue'
import type { CombinedView, DownloadState, DownloadView, DownloadsView, LinkView, TableView } from '../api/types'
import {
  averagePing,
  clock,
  countedNote,
  eta,
  fetched,
  gaugeCaption,
  heroFigure,
  leadTransfer,
  linkedCables,
  notCountedText,
  refusalLines,
  stoppable,
} from './figures'

let nextId = 1

function link(name: string, fields: Partial<LinkView> = {}): LinkView {
  return {
    id: nextId++,
    name,
    address: `192.0.2.${nextId}`,
    family: 'ipv4',
    state: 'up',
    rtt_ms: 40,
    down_bps: null,
    public_ip: null,
    metered: false,
    ...fields,
  }
}

const NOTHING: CombinedView = { bps: 0, parts: [], not_counted: [] }

function table(links: LinkView[], combined: CombinedView = NOTHING): TableView {
  return { links, shared: [], connections: [], combined }
}

function download(id: number, state: DownloadState, fields: Partial<DownloadView> = {}): DownloadView {
  return {
    id,
    url: `https://example.com/file-${id}.iso`,
    name: `file-${id}.iso`,
    state,
    length: 1000,
    done: 300,
    bps: 0,
    note: null,
    cables: [],
    ...fields,
  }
}

function downloads(list: DownloadView[], fields: Partial<DownloadsView> = {}): DownloadsView {
  return { live: { bps: 0, parts: [] }, list, used: [], ...fields }
}

const TWO: CombinedView = {
  bps: 57_000_000,
  parts: [
    { name: 'Ethernet', bps: 30_000_000, shared_with: [] },
    { name: 'Wi-Fi', bps: 27_000_000, shared_with: [] },
  ],
  not_counted: [],
}

test('the figure is the live speed while anything downloads, else the combined speed available', () => {
  const t = table([link('Wi-Fi')], TWO)
  const live = downloads([download(1, 'downloading')], {
    live: { bps: 34_800_000, parts: [{ name: 'Wi-Fi', bps: 34_800_000 }] },
  })
  expect(heroFigure(t, live)).toEqual({
    mode: 'live',
    bps: 34_800_000,
    parts: [{ name: 'Wi-Fi', bps: 34_800_000 }],
    running: 1,
    available: 57_000_000,
  })

  // Waiting and starting do not count as downloading: the figure stays on what is available.
  const idle = heroFigure(t, downloads([download(1, 'waiting'), download(2, 'starting')]))
  expect(idle.mode).toBe('available')
  expect(idle.bps).toBe(57_000_000)
  expect(idle.parts.map((p) => p.name)).toEqual(['Ethernet', 'Wi-Fi'])
})

test('nothing measured: the figure is unknown, never 0', () => {
  expect(heroFigure(table([link('Wi-Fi', { state: 'probing' })]), null).bps).toBeNull()
  expect(heroFigure(null, null).bps).toBeNull()
  // A stalled download is a real 0.
  const stalled = heroFigure(table([]), downloads([download(1, 'downloading')]))
  expect(stalled).toMatchObject({ mode: 'live', bps: 0 })
})

test('the caption under the figure says what it is out of', () => {
  const t = table([link('Wi-Fi')], TWO)
  const live = (bps: number, parts = 1) =>
    heroFigure(t, downloads([download(1, 'downloading')], {
      live: { bps, parts: Array.from({ length: parts }, (_, i) => ({ name: `C${i}`, bps: bps / parts })) },
    }))
  expect(gaugeCaption(live(34_800_000))).toBe('61% of 57.0 Mbps')
  expect(gaugeCaption(live(0, 0))).toBe('1 download, nothing arriving right now')

  const unmeasured = heroFigure(table([]), downloads([download(1, 'downloading'), download(2, 'downloading')], {
    live: { bps: 9_000_000, parts: [{ name: 'Wi-Fi', bps: 4_000_000 }, { name: 'Ethernet', bps: 5_000_000 }] },
  }))
  expect(gaugeCaption(unmeasured)).toBe('2 downloads, through 2 cables')

  expect(gaugeCaption(heroFigure(t, null))).toBe('from 2 independent connections')
  expect(gaugeCaption(heroFigure(table([link('Wi-Fi')], { ...TWO, parts: [TWO.parts[0]] }), null))).toBe(
    'from 1 connection',
  )
  expect(gaugeCaption(heroFigure(table([link('Wi-Fi', { state: 'probing' })]), null))).toBe('nothing measured yet')
})

test('linked cables are the ones up or degraded, each counted once', () => {
  const t = table([
    link('Wi-Fi'),
    link('Wi-Fi', { state: 'down' }),
    link('Ethernet', { state: 'degraded' }),
    link('Ethernet 2', { state: 'portal' }),
    link('Bluetooth Network', { state: 'down' }),
  ])
  expect(linkedCables(t)).toBe(2)
  expect(linkedCables(null)).toBeNull()
})

test('the average ping is over the usable cables that have one, "-" when none', () => {
  const t = table([
    link('Wi-Fi', { rtt_ms: 12 }),
    link('Ethernet', { rtt_ms: 5 }),
    link('Ethernet 2', { state: 'degraded', rtt_ms: 39 }),
    link('Bluetooth Network', { state: 'down', rtt_ms: 900 }),
  ])
  expect(averagePing(t)).toBe(19)
  expect(averagePing(table([link('Wi-Fi', { state: 'probing', rtt_ms: null })]))).toBeNull()
  expect(averagePing(null)).toBeNull()
})

test('fetched is every byte the cables have received', () => {
  const used = [
    { cable: 'Wi-Fi', bytes: 1_200_000 },
    { cable: 'Ethernet', bytes: 300_000 },
  ]
  expect(fetched(downloads([], { used }))).toBe(1_500_000)
  expect(fetched(null)).toBeNull()
})

test('the lead transfer is the downloading download with the most left to fetch', () => {
  const list = [
    download(1, 'downloading', { length: 1000, done: 900 }),
    download(2, 'downloading', { length: 5000, done: 1000 }),
    download(3, 'waiting', { length: 90_000, done: 0 }),
    download(4, 'downloading', { length: null, done: 10 }),
  ]
  expect(leadTransfer(downloads(list))?.id).toBe(2)
  // Only sizes unknown: the first downloading one.
  expect(leadTransfer(downloads([download(4, 'downloading', { length: null })]))?.id).toBe(4)
  expect(leadTransfer(downloads([download(1, 'done')]))).toBeNull()
  expect(leadTransfer(null)).toBeNull()
})

test('the time left is written in its two largest units, "-" when it cannot be known', () => {
  expect(eta(download(1, 'downloading', { length: 8_000_000, done: 0, bps: 8_000_000 }))).toBe('8s')
  expect(eta(download(1, 'downloading', { length: 712_000_000, done: 0, bps: 8_000_000 }))).toBe('11m 52s')
  expect(eta(download(1, 'downloading', { length: 4_000_000_000, done: 0, bps: 8_000_000 }))).toBe('1h 06m')
  expect(eta(download(1, 'downloading', { bps: 0 }))).toBe('-')
  expect(eta(download(1, 'downloading', { length: null, bps: 8_000_000 }))).toBe('-')
})

test("the lead transfer's time left reads the same as its card's, a day or more included", () => {
  for (const length of [8_000_000, 712_000_000, 4_000_000_000, 94_000_000_000, 900_000_000_000]) {
    const d = download(1, 'downloading', { length, done: 0, bps: 8_000_000 })
    expect(eta(d), `${length} bytes`).toBe(etaText(d))
  }
  expect(eta(download(1, 'downloading', { length: 94_000_000_000, done: 0, bps: 8_000_000 }))).toBe('1d 2h')
})

test('the connected clock', () => {
  expect(clock(0)).toBe('00:00:00')
  expect(clock(216_400)).toBe('00:03:36')
  expect(clock(37 * 3_600_000 + 61_000)).toBe('37:01:01')
})

test('the cables left out of the sum, named with why', () => {
  expect(notCountedText([{ name: 'Bluetooth Network', why: 'down' }])).toBe('1 cable not counted: Bluetooth Network (down)')
  expect(
    notCountedText([
      { name: 'Ethernet 4', why: 'down' },
      { name: 'Ethernet 5', why: 'not measured yet' },
    ]),
  ).toBe('2 cables not counted: Ethernet 4 (down), Ethernet 5 (not measured yet)')
})

test('the note says what was counted, what counted once and what was left out', () => {
  const combined: CombinedView = {
    bps: 57_600_000,
    parts: [{ name: 'Wi-Fi', bps: 57_600_000, shared_with: ['Ethernet 3'] }],
    not_counted: [{ name: 'Ethernet 4', why: 'down' }],
  }
  expect(countedNote(combined)).toBe(
    "The sum of each working connection's measured speed. Ethernet 3 shares Wi-Fi's connection, so it is counted once with it. Not counted: Ethernet 4 (down).",
  )
})

test('stop all asks every download that is starting, waiting or downloading', () => {
  const list = (['starting', 'waiting', 'downloading', 'done', 'failed', 'stopped'] as const).map((s, i) => download(i + 1, s))
  expect(stoppable(downloads(list)).map((d) => d.id)).toEqual([1, 2, 3])
  expect(stoppable(null)).toEqual([])
})

test("Stop all's refusals: each wording once, with the downloads it was for unless it was for all of them", () => {
  expect(refusalLines([{ name: 'a.iso', words: 'download 1 is not running' }], 1)).toEqual([
    'a.iso: download 1 is not running',
  ])
  const off = 'the engine is not answering'
  expect(refusalLines([{ name: 'a.iso', words: off }, { name: 'b.iso', words: off }], 2)).toEqual([`2 downloads: ${off}`])
  expect(
    refusalLines(
      [
        { name: 'a.iso', words: 'download 1 is not running' },
        { name: 'b.iso', words: off },
        { name: 'c.iso', words: off },
      ],
      4,
    ),
  ).toEqual(['a.iso: download 1 is not running', `b.iso, c.iso: ${off}`])
  expect(refusalLines([], 3)).toEqual([])
})
