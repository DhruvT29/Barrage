import { fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, expect, test } from 'vitest'

import { EMPTY_HISTORY } from '../api/history'
import type { DownloadState, DownloadView, DownloadsView, TorrentsView, TorrentPhase, TorrentView } from '../api/types'
import { forgetChoices } from './queue'
import { TransferQueue } from './TransferQueue'

afterEach(() => {
  forgetChoices()
})

const NOTHING: DownloadsView = { live: { bps: 0, parts: [] }, list: [], used: [] }

function download(id: number, name: string, state: DownloadState = 'done'): DownloadView {
  return {
    id,
    url: `https://example.com/${name}`,
    name,
    state,
    length: 1000,
    done: state === 'done' ? 1000 : 400,
    bps: 0,
    note: null,
    cables: [{ name: 'Wi-Fi', bytes: state === 'done' ? 1000 : 400, bps: 0, connections: 0, wanted: 0, notes: [] }],
  }
}

function torrent(id: number, name: string, phase: TorrentPhase = 'done'): TorrentView {
  return {
    id,
    name,
    info_hash: String(id).repeat(40).slice(0, 40),
    phase,
    length: 1000,
    done: 400,
    down_bps: 0,
    up_bps: 0,
    peers: 0,
    note: null,
    cables: [],
  }
}

function swarm(...list: TorrentView[]): TorrentsView {
  return { list, udp_cable: 'Wi-Fi', listening: { state: 'trying', cable: 'Wi-Fi', why: null } }
}

function show(downloads: DownloadsView | null, onAdd = () => {}, torrents: TorrentsView | null = null) {
  return render(
    <TransferQueue
      status="live"
      table={null}
      feed={[]}
      downloads={downloads}
      torrents={torrents}
      history={EMPTY_HISTORY}
      colors={{ 'Wi-Fi': '#22ff66' }}
      onAdd={onAdd}
    />,
  )
}

/** The download cards, by name, in the order shown. */
const cards = () =>
  screen
    .getAllByRole('region')
    .filter((r) => r.classList.contains('tq-card'))
    .map((r) => r.getAttribute('aria-label'))

const filters = () => within(screen.getByRole('group', { name: 'Show' })).getAllByRole('button')
const filter = (name: RegExp) => within(screen.getByRole('group', { name: 'Show' })).getByRole('button', { name })

const MIXED: DownloadsView = {
  ...NOTHING,
  list: [
    download(1, 'a.iso', 'done'),
    download(2, 'b.iso', 'downloading'),
    download(3, 'c.iso', 'stopped'),
    download(4, 'd.iso', 'waiting'),
    download(5, 'e.iso', 'starting'),
    download(6, 'f.iso', 'done'),
  ],
}

test('the heading says how many downloads there are, in green, two digits', () => {
  show(MIXED)
  const queue = screen.getByRole('region', { name: 'Transfer queue' })
  expect(within(queue).getByRole('heading', { level: 2 })).toHaveTextContent(/^Queue 06$/)
  expect(within(queue).getByTestId('count')).toHaveTextContent(/^06$/)
})

test('the newest download comes first', () => {
  const list = [download(1, 'a.iso'), download(2, 'b.iso'), download(3, 'c.iso')]
  show({ ...NOTHING, list })
  expect(cards()).toEqual(['c.iso', 'b.iso', 'a.iso'])
})

test('the filters count honestly, and one with nothing in it is left out, except All and Active', () => {
  show(MIXED)
  // Active is starting, waiting and downloading; nothing failed, so no Failed.
  expect(filters().map((b) => b.textContent)).toEqual(['All 6', 'Active 3', 'Stopped 1', 'Done 2'])
  expect(filter(/^All/)).toHaveAttribute('aria-pressed', 'true')
  expect(filter(/^Active/)).toHaveAttribute('aria-pressed', 'false')
})

test('All and Active show even at 0', () => {
  show({ ...NOTHING, list: [download(1, 'a.iso', 'failed')] })
  expect(filters().map((b) => b.textContent)).toEqual(['All 1', 'Active 0', 'Failed 1'])
})

test('a filter shows only its downloads, newest first, and All shows them all again', () => {
  show(MIXED)
  fireEvent.click(filter(/^Active/))
  expect(filter(/^Active/)).toHaveAttribute('aria-pressed', 'true')
  expect(filter(/^All/)).toHaveAttribute('aria-pressed', 'false')
  expect(cards()).toEqual(['e.iso', 'd.iso', 'b.iso'])

  fireEvent.click(filter(/^Done/))
  expect(cards()).toEqual(['f.iso', 'a.iso'])

  fireEvent.click(filter(/^Stopped/))
  expect(cards()).toEqual(['c.iso'])

  fireEvent.click(filter(/^All/))
  expect(cards()).toHaveLength(6)
})

test('a chosen filter that empties stays, and says so', () => {
  const { rerender } = show(MIXED)
  fireEvent.click(filter(/^Stopped/))
  // Resumed: nothing is stopped any more.
  const resumed = { ...MIXED, list: MIXED.list.map((d) => (d.id === 3 ? { ...d, state: 'downloading' as const } : d)) }
  rerender(
    <TransferQueue
      status="live"
      table={null}
      feed={[]}
      downloads={resumed}
      history={EMPTY_HISTORY}
      colors={{}}
      onAdd={() => {}}
    />,
  )
  expect(filter(/^Stopped/)).toHaveTextContent('Stopped 0')
  expect(filter(/^Stopped/)).toHaveAttribute('aria-pressed', 'true')
  expect(screen.queryAllByRole('region').filter((r) => r.classList.contains('tq-card'))).toHaveLength(0)
  expect(screen.getByText('Nothing stopped.')).toBeInTheDocument()
  // Nothing to add from a filter: the add button is for an empty queue.
  expect(screen.queryByRole('button', { name: /add download/i })).toBeNull()
})

test('an empty Active says so', () => {
  show({ ...NOTHING, list: [download(1, 'a.iso')] })
  fireEvent.click(filter(/^Active/))
  expect(screen.getByText('Nothing active.')).toBeInTheDocument()
})

test('no downloads yet: says so, and the button opens the add window', () => {
  let asked = 0
  show(NOTHING, () => (asked += 1))
  expect(screen.getByText('Nothing downloaded yet.')).toBeInTheDocument()
  expect(screen.getByTestId('count')).toHaveTextContent(/^00$/)
  fireEvent.click(screen.getByRole('button', { name: '+ Add download' }))
  expect(asked).toBe(1)
})

test('before the engine has said anything: no cards, no filters and no empty-list message', () => {
  show(null)
  expect(screen.queryByText(/Nothing downloaded yet/)).toBeNull()
  expect(screen.queryByRole('button', { name: /add download/i })).toBeNull()
  expect(screen.queryByRole('group', { name: 'Show' })).toBeNull()
  expect(screen.getByTestId('count')).toHaveTextContent(/^-$/)
  expect(cards()).toEqual([])
})

const toggleOf = (name: string) =>
  within(screen.getByRole('region', { name })).getByRole('button', { name: /^Link distribution/ })

test("the user's choice to open or close a card's cables is kept per download, across filters and redraws", () => {
  const { unmount } = show(MIXED)
  // b.iso is downloading: open. a.iso is done: closed.
  expect(toggleOf('b.iso')).toHaveAttribute('aria-expanded', 'true')
  expect(toggleOf('a.iso')).toHaveAttribute('aria-expanded', 'false')
  fireEvent.click(toggleOf('b.iso'))
  fireEvent.click(toggleOf('a.iso'))

  // Filtered away and back: the cards are drawn anew, the choice stands.
  fireEvent.click(filter(/^Stopped/))
  fireEvent.click(filter(/^All/))
  expect(toggleOf('b.iso')).toHaveAttribute('aria-expanded', 'false')
  expect(toggleOf('a.iso')).toHaveAttribute('aria-expanded', 'true')
  // Another download's is its own.
  expect(toggleOf('f.iso')).toHaveAttribute('aria-expanded', 'false')

  // Away to the radar and back, the page still open: still kept.
  unmount()
  show(MIXED)
  expect(toggleOf('b.iso')).toHaveAttribute('aria-expanded', 'false')
  expect(toggleOf('a.iso')).toHaveAttribute('aria-expanded', 'true')
})

const titles = () =>
  within(screen.getByRole('region', { name: 'Transfer queue' }))
    .getAllByRole('region')
    .map((card) => card.getAttribute('aria-label'))

const chip = (name: RegExp) => screen.getByRole('button', { name })

test('torrents sit above downloads, each the newest first, and both are counted', () => {
  const downloads = { ...NOTHING, list: [download(1, 'a.zip'), download(2, 'b.zip')] }
  show(downloads, () => {}, swarm(torrent(1, 'x.iso'), torrent(2, 'y.iso')))
  expect(titles()).toEqual(['y.iso', 'x.iso', 'b.zip', 'a.zip'])
  expect(screen.getByTestId('count')).toHaveTextContent('04')
})

test('the Torrents filter shows only torrents', () => {
  show({ ...NOTHING, list: [download(1, 'a.zip')] }, () => {}, swarm(torrent(1, 'x.iso')))
  fireEvent.click(chip(/^Torrents/))
  expect(titles()).toEqual(['x.iso'])
})

test('the other filters take torrents by their phase', () => {
  show(
    { ...NOTHING, list: [download(1, 'a.zip', 'stopped')] },
    () => {},
    swarm(torrent(1, 'x.iso', 'downloading'), torrent(2, 'y.iso', 'stopped')),
  )
  fireEvent.click(chip(/^Stopped/))
  expect(titles()).toEqual(['y.iso', 'a.zip'])
  fireEvent.click(chip(/^Active/))
  expect(titles()).toEqual(['x.iso'])
})

test('a queue of nothing but torrents offers no add, and its filters have something to say', () => {
  show(NOTHING, () => {}, swarm(torrent(1, 'x.iso', 'done')))
  expect(screen.queryByText('Nothing downloaded yet.')).toBeNull()
  fireEvent.click(chip(/^Active/))
  expect(screen.getByText('Nothing active.')).toBeInTheDocument()
})

test('the swarm line shows once there is a torrent', () => {
  const { unmount } = show(NOTHING, () => {}, swarm())
  expect(screen.queryByTestId('swarm')).toBeNull()
  unmount()
  show(NOTHING, () => {}, swarm(torrent(1, 'x.iso')))
  expect(screen.getByTestId('swarm')).toHaveTextContent('DHT and UDP on Wi-Fi')
})

test('a torrent and a download of one number each keep their own open or closed', () => {
  const stopped = { ...download(1, 'a.zip', 'stopped') }
  const held = torrent(1, 'x.iso', 'stopped')
  held.cables = [{ name: 'Wi-Fi', received: 400, sent: 0, bps: 0, connections: 0, uploads: true }]
  show({ ...NOTHING, list: [stopped] }, () => {}, swarm(held))
  const toggle = (name: string) =>
    within(screen.getByRole('region', { name })).getByRole('button', { name: /Link distribution/ })
  fireEvent.click(toggle('x.iso'))
  expect(toggle('x.iso')).toHaveAttribute('aria-expanded', 'true')
  expect(toggle('a.zip')).toHaveAttribute('aria-expanded', 'false')
})
