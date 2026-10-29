import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'

import type { DownloadsView, LinkView, LiveMessage, TableView } from './types'
import { FEED_MAX, RETRY_MS, useLive } from './useLive'

/** A socket the test drives by hand, standing in for the engine. */
class FakeSocket {
  onmessage: ((event: { data: string }) => void) | null = null
  onerror: (() => void) | null = null
  onclose: (() => void) | null = null
  closed = false
  readonly url: string

  constructor(url: string) {
    this.url = url
  }

  /** As in a browser, closing on purpose still fires `close`, a moment later. */
  close() {
    this.closed = true
    setTimeout(() => this.onclose?.(), 0)
  }

  /** The engine sends a message. */
  send(message: LiveMessage) {
    act(() => this.onmessage?.({ data: JSON.stringify(message) }))
  }

  /** The connection is lost, the way browsers report it: an error, then a close. */
  drop() {
    this.closed = true
    act(() => {
      this.onerror?.()
      this.onclose?.()
    })
  }
}

/** Every socket the hook asked for, in order, and the `open` that makes them. */
function engine() {
  const sockets: FakeSocket[] = []
  const open = (url: string) => {
    const socket = new FakeSocket(url)
    sockets.push(socket)
    return socket as unknown as WebSocket
  }
  const last = () => sockets[sockets.length - 1]
  const openCount = () => sockets.filter((s) => !s.closed).length
  return { sockets, open, last, openCount }
}

const URL = 'ws://127.0.0.1:7700/api/live'

function link(id: number, name: string): LinkView {
  return {
    id,
    name,
    address: `192.0.2.${id}`,
    family: 'ipv4',
    state: 'up',
    rtt_ms: 40,
    down_bps: 57_600_000,
    public_ip: '203.0.113.7',
  }
}

function table(...links: LinkView[]): TableView {
  return { links, shared: [], connections: [], combined: { bps: 0, parts: [], not_counted: [] } }
}

const after = (ms: number) => act(() => vi.advanceTimersByTime(ms))

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(new Date('2026-10-14T09:30:00Z'))
})

afterEach(() => {
  vi.useRealTimers()
})

test('the first table makes it live', () => {
  const e = engine()
  const { result } = renderHook(() => useLive(URL, e.open))
  expect(result.current).toEqual({ status: 'connecting', table: null, feed: [], downloads: null })
  expect(e.last().url).toBe(URL)

  e.last().send({ type: 'table', table: table(link(1, 'Wi-Fi')) })

  expect(result.current.status).toBe('live')
  expect(result.current.table).toEqual(table(link(1, 'Wi-Fi')))
  expect(result.current.feed).toEqual([])
})

test('a change replaces the table and adds its lines to the top of the feed', () => {
  const e = engine()
  const { result } = renderHook(() => useLive(URL, e.open))
  e.last().send({ type: 'table', table: table(link(1, 'Wi-Fi')) })

  e.last().send({
    type: 'change',
    table: table(link(1, 'Wi-Fi'), link(2, 'Ethernet 2')),
    lines: ['+ Ethernet 2 192.0.2.2', 'Ethernet 2 192.0.2.2 probing -> up'],
  })
  vi.setSystemTime(new Date('2026-10-14T09:30:05Z'))
  e.last().send({ type: 'change', table: table(link(2, 'Ethernet 2')), lines: ['- Wi-Fi 192.0.2.1'] })

  expect(result.current.table).toEqual(table(link(2, 'Ethernet 2')))
  expect(result.current.feed).toEqual([
    { at: new Date('2026-10-14T09:30:05Z'), text: '- Wi-Fi 192.0.2.1' },
    { at: new Date('2026-10-14T09:30:00Z'), text: '+ Ethernet 2 192.0.2.2' },
    { at: new Date('2026-10-14T09:30:00Z'), text: 'Ethernet 2 192.0.2.2 probing -> up' },
  ])
})

/** One download running through Wi-Fi. */
const RUNNING: DownloadsView = {
  live: { bps: 1_000_000, parts: [{ name: 'Wi-Fi', bps: 1_000_000 }] },
  list: [
    {
      id: 1,
      url: 'https://example.com/a.iso',
      name: 'a.iso',
      state: 'downloading',
      length: 1000,
      done: 300,
      bps: 1_000_000,
      note: null,
      cables: [{ name: 'Wi-Fi', bytes: 300, bps: 1_000_000, connections: 4, notes: [] }],
    },
  ],
}

test('a downloads message replaces the downloads, and leaves the table and the feed alone', () => {
  const e = engine()
  const { result } = renderHook(() => useLive(URL, e.open))
  e.last().send({ type: 'table', table: table(link(1, 'Wi-Fi')) })
  e.last().send({ type: 'change', table: table(link(1, 'Wi-Fi')), lines: ['a line'] })
  const { table: before, feed } = result.current

  e.last().send({ type: 'downloads', downloads: { live: { bps: 0, parts: [] }, list: [] } })
  expect(result.current.downloads).toEqual({ live: { bps: 0, parts: [] }, list: [] })
  e.last().send({ type: 'downloads', downloads: RUNNING })
  expect(result.current.downloads).toEqual(RUNNING)

  expect(result.current.table).toBe(before)
  expect(result.current.feed).toBe(feed)
  expect(result.current.status).toBe('live')
})

test('a closed socket shows disconnected and keeps the last table and downloads', () => {
  const e = engine()
  const { result } = renderHook(() => useLive(URL, e.open))
  e.last().send({ type: 'table', table: table(link(1, 'Wi-Fi')) })
  e.last().send({ type: 'downloads', downloads: RUNNING })
  e.last().send({ type: 'change', table: table(link(1, 'Wi-Fi')), lines: ['a line'] })

  e.last().drop()

  expect(result.current.status).toBe('disconnected')
  expect(result.current.table).toEqual(table(link(1, 'Wi-Fi')))
  expect(result.current.feed.map((l) => l.text)).toEqual(['a line'])
  expect(result.current.downloads).toEqual(RUNNING)
})

test('it reconnects after 1 s, then 2 s, 4 s and 5 s, and resets on success', () => {
  expect(RETRY_MS).toEqual([1000, 2000, 4000, 5000])
  const e = engine()
  const { result } = renderHook(() => useLive(URL, e.open))

  // The engine is not running yet: each attempt fails, and waits longer.
  for (const [i, wait] of [1000, 2000, 4000, 5000, 5000, 5000].entries()) {
    e.last().drop()
    after(wait - 1)
    expect(e.sockets).toHaveLength(i + 1)
    after(1)
    expect(e.sockets).toHaveLength(i + 2)
    expect(e.last().url).toBe(URL)
  }
  expect(result.current.status).toBe('disconnected')

  // It starts: the table arrives, and the next loss waits 1 s again.
  e.last().send({ type: 'table', table: table(link(1, 'Wi-Fi')) })
  expect(result.current.status).toBe('live')
  const count = e.sockets.length
  e.last().drop()
  after(999)
  expect(e.sockets).toHaveLength(count)
  after(1)
  expect(e.sockets).toHaveLength(count + 1)
})

test('a socket that opens but sends nothing does not reset the wait', () => {
  const e = engine()
  renderHook(() => useLive(URL, e.open))
  e.last().drop()
  after(1000)

  // Connected, no table, lost again: the next wait is 2 s, not 1 s.
  e.last().drop()
  after(1999)
  expect(e.sockets).toHaveLength(2)
  after(1)
  expect(e.sockets).toHaveLength(3)
})

test('a reconnect shows the new table, nothing merged, and keeps the feed', () => {
  const e = engine()
  const { result } = renderHook(() => useLive(URL, e.open))
  e.last().send({ type: 'table', table: table(link(1, 'Wi-Fi'), link(2, 'Ethernet 2')) })
  e.last().send({ type: 'change', table: table(link(1, 'Wi-Fi'), link(2, 'Ethernet 2')), lines: ['before'] })
  e.last().drop()
  after(1000)

  e.last().send({ type: 'table', table: table(link(7, 'Wi-Fi')) })

  expect(result.current.status).toBe('live')
  expect(result.current.table).toEqual(table(link(7, 'Wi-Fi')))
  expect(result.current.feed.map((l) => l.text)).toEqual(['before'])
})

test('there is never more than one socket open at a time', () => {
  const e = engine()
  renderHook(() => useLive(URL, e.open))

  for (let i = 0; i < 20; i++) {
    expect(e.openCount()).toBe(1)
    e.last().drop()
    expect(e.openCount()).toBe(0)
    after(5000)
  }
  // An error and a close for one loss are one retry, not two.
  expect(e.sockets).toHaveLength(21)
  expect(e.openCount()).toBe(1)
})

test('the feed keeps the newest 200 lines', () => {
  expect(FEED_MAX).toBe(200)
  const e = engine()
  const { result } = renderHook(() => useLive(URL, e.open))
  e.last().send({ type: 'table', table: table() })

  for (let batch = 0; batch < 5; batch++) {
    const lines = Array.from({ length: 50 }, (_, i) => `line ${batch * 50 + i}`)
    e.last().send({ type: 'change', table: table(), lines })
  }

  const texts = result.current.feed.map((l) => l.text)
  expect(texts).toHaveLength(200)
  expect(texts[0]).toBe('line 200')
  expect(texts[49]).toBe('line 249')
  expect(texts[199]).toBe('line 99')
})

test('leaving the page closes the socket and cancels a pending retry', () => {
  // Connected: the socket is closed, and its late `close` event is not a loss.
  const live = engine()
  const first = renderHook(() => useLive(URL, live.open))
  first.unmount()
  expect(live.last().closed).toBe(true)
  after(60_000)
  expect(live.sockets).toHaveLength(1)

  // Waiting to retry: the retry never happens.
  const waiting = engine()
  const second = renderHook(() => useLive(URL, waiting.open))
  waiting.last().drop()
  second.unmount()
  after(60_000)
  expect(waiting.sockets).toHaveLength(1)
})

test('by default it connects to the engine that served the page', () => {
  const e = engine()
  renderHook(() => useLive(undefined, e.open))
  expect(e.last().url).toBe(`ws://${window.location.host}/api/live`)
})
