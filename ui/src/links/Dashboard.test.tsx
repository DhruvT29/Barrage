import { fireEvent, render, screen, within } from '@testing-library/react'
import { expect, test } from 'vitest'

import type { FeedLine, Status } from '../api/useLive'
import type { CombinedView, DownloadState, DownloadView, DownloadsView, LinkView, TableView } from '../api/types'
import type { Tab } from '../tabs'
import { Dashboard } from './Dashboard'

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
    ...fields,
  }
}

const NOTHING: CombinedView = { bps: 0, parts: [], not_counted: [] }

function table(links: LinkView[], fields: Partial<TableView> = {}): TableView {
  return { links, shared: [], connections: [], combined: NOTHING, ...fields }
}

type More = { downloads: DownloadsView | null; tab: Tab; onTab: (tab: Tab) => void }

function page(status: Status, t: TableView | null, feed: FeedLine[] = [], more: Partial<More> = {}) {
  return <Dashboard status={status} table={t} feed={feed} downloads={null} tab="links" onTab={() => {}} {...more} />
}

function show(status: Status, t: TableView | null, feed: FeedLine[] = [], more: Partial<More> = {}) {
  return render(page(status, t, feed, more))
}

function download(id: number, state: DownloadState): DownloadView {
  return {
    id,
    url: `https://example.com/file-${id}.iso`,
    name: `file-${id}.iso`,
    state,
    length: 1000,
    done: state === 'done' ? 1000 : 300,
    bps: 0,
    note: null,
    cables: [],
  }
}

/** One download running through two cables, one finished before it. */
const LIVE: DownloadsView = {
  live: {
    bps: 189_900_000,
    parts: [
      { name: 'Wi-Fi', bps: 48_200_000 },
      { name: 'Ethernet 2', bps: 141_700_000 },
    ],
  },
  list: [download(1, 'done'), download(2, 'downloading')],
}

const hero = () => screen.getByRole('region', { name: 'Combined speed available' })

const wifi = () => link('Wi-Fi', '192.0.2.10', { down_bps: 57_600_000, public_ip: '203.0.113.7' })
const eth2 = () => link('Ethernet 2', '198.51.100.20', { down_bps: 162_500_000, public_ip: '203.0.113.9' })

const TWO: CombinedView = {
  bps: 220_100_000,
  parts: [
    { name: 'Wi-Fi', bps: 57_600_000, shared_with: [] },
    { name: 'Ethernet 2', bps: 162_500_000, shared_with: [] },
  ],
  not_counted: [],
}

test('the combined speed leads the page, with a segment per independent connection', () => {
  show('live', table([wifi(), eth2()], { combined: TWO }))

  const h = hero()
  // First in the content, before any card.
  const regions = screen.getAllByRole('region')
  expect(regions[0]).toBe(h)
  expect(within(h).getByTestId('total')).toHaveTextContent('220.1 Mbps')
  expect(h).toHaveTextContent('from 2 independent connections')
  const segments = within(h).getAllByTestId('segment')
  expect(segments.map((s) => parseFloat(s.style.width))).toEqual([expect.closeTo(26.17, 2), expect.closeTo(73.83, 2)])
  expect(within(h).getAllByTestId('part').map((p) => p.textContent)).toEqual(['Wi-Fi 57.6', 'Ethernet 2 162.5'])
})

test('one connection is "from 1 connection"', () => {
  show('live', table([wifi()], { combined: { bps: 57_600_000, parts: [TWO.parts[0]], not_counted: [] } }))
  expect(hero()).toHaveTextContent('from 1 connection')
  expect(hero()).not.toHaveTextContent('independent')
})

test('the hero says what was counted once and what was not counted', () => {
  const combined: CombinedView = {
    bps: 57_600_000,
    parts: [{ name: 'Wi-Fi', bps: 57_600_000, shared_with: ['Ethernet 3'] }],
    not_counted: [
      { name: 'Ethernet 4', why: 'down' },
      { name: 'Ethernet 5', why: 'not measured yet' },
    ],
  }
  show('live', table([wifi()], { combined }))

  const note = within(hero()).getByTestId('note')
  expect(note).toHaveTextContent("Ethernet 3 shares Wi-Fi's connection, so it is counted once with it.")
  expect(note).toHaveTextContent('Not counted: Ethernet 4 (down), Ethernet 5 (not measured yet).')
})

test('nothing measured yet: the total is "-", never 0', () => {
  show('live', table([link('Wi-Fi', '192.0.2.10', { state: 'probing' })], {
    combined: { bps: 0, parts: [], not_counted: [{ name: 'Wi-Fi', why: 'probing' }] },
  }))
  expect(within(hero()).getByTestId('total')).toHaveTextContent(/^-$/)
  expect(hero()).toHaveTextContent('nothing measured yet')
  expect(within(hero()).queryAllByTestId('segment')).toHaveLength(0)
})

test('a card per cable', () => {
  show('live', table([wifi(), eth2(), link('Wi-Fi', '2001:db8::10')], { combined: TWO }))
  expect(screen.getByRole('region', { name: 'Wi-Fi' })).toBeInTheDocument()
  expect(screen.getByRole('region', { name: 'Ethernet 2' })).toBeInTheDocument()
  expect(screen.getAllByRole('region')).toHaveLength(3) // the hero and two cables
})

test("the cards share one scale: the fastest cable's beam is full", () => {
  show('live', table([wifi(), eth2()], { combined: TWO }))
  const beam = (name: string) => within(screen.getByRole('region', { name })).getByTestId('beam-fill')
  expect(beam('Ethernet 2')).toHaveStyle({ width: '100%' })
  expect(parseFloat(beam('Wi-Fi').style.width)).toBeCloseTo(35.45, 2)
})

test('the shared notice names both links and why', () => {
  const a = wifi()
  // A second address on Wi-Fi behind the same public address: Wi-Fi is named once.
  const a2 = link('Wi-Fi', '192.0.2.11', { public_ip: '203.0.113.7' })
  const b = link('Ethernet 2', '198.51.100.20', { public_ip: '203.0.113.7' })
  show('live', table([a, a2, b], { shared: [[a.id, a2.id, b.id]] }))

  expect(screen.getByTestId('shared-notice')).toHaveTextContent(
    'One connection. Wi-Fi and Ethernet 2 reach the internet through one connection (203.0.113.7), so using both gains nothing.',
  )
})

test('no shared groups, no notice', () => {
  show('live', table([wifi(), eth2()], { combined: TWO }))
  expect(screen.queryByTestId('shared-notice')).toBeNull()
})

test('no links: says so and how to get one, keeps running', () => {
  show('live', table([]))
  expect(screen.getByText(/No internet connections\./)).toHaveTextContent(
    'No internet connections. Join a Wi-Fi network, or plug in a phone with USB tethering on.',
  )
  expect(screen.getByTestId('light')).toHaveTextContent('live')
})

test('disconnected: says so and still shows the last table, dimmed as old', () => {
  const t = table([wifi()], { combined: { bps: 57_600_000, parts: [TWO.parts[0]], not_counted: [] } })
  const { container, rerender } = show('disconnected', t)
  expect(screen.getByTestId('light')).toHaveTextContent('disconnected, retrying')
  expect(screen.getByRole('region', { name: 'Wi-Fi' })).toBeInTheDocument()
  expect(within(hero()).getByTestId('total')).toHaveTextContent('57.6 Mbps')
  expect(container.querySelector('.layout')).toHaveClass('stale')

  rerender(page('live', t))
  expect(container.querySelector('.layout')).not.toHaveClass('stale')
})

test('while anything downloads, the hero shows the live download speed per cable', () => {
  show('live', table([wifi(), eth2()], { combined: TWO }), [], { downloads: LIVE })

  const h = screen.getByRole('region', { name: 'Downloading at' })
  expect(screen.getAllByRole('region')[0]).toBe(h)
  expect(within(h).getByTestId('total')).toHaveTextContent('189.9 Mbps')
  expect(h).toHaveTextContent('1 download, through 2 cables')
  const segments = within(h).getAllByTestId('segment')
  expect(segments.map((s) => parseFloat(s.style.width))).toEqual([expect.closeTo(25.38, 2), expect.closeTo(74.62, 2)])
  expect(within(h).getAllByTestId('part').map((p) => p.textContent)).toEqual(['Wi-Fi 48.2', 'Ethernet 2 141.7'])
  expect(within(h).getByTestId('note')).toHaveTextContent(
    'What every download is fetching right now, added up by cable. With nothing downloading, this shows the combined speed available.',
  )
  expect(screen.queryByRole('region', { name: 'Combined speed available' })).toBeNull()
})

test('with nothing downloading, the hero is the combined speed available, as before', () => {
  const ended: DownloadsView = {
    live: { bps: 0, parts: [] },
    list: [download(1, 'done'), download(2, 'failed'), download(3, 'waiting'), download(4, 'starting')],
  }
  show('live', table([wifi(), eth2()], { combined: TWO }), [], { downloads: ended })

  expect(within(hero()).getByTestId('total')).toHaveTextContent('220.1 Mbps')
  expect(hero()).toHaveTextContent('from 2 independent connections')
  expect(screen.queryByRole('region', { name: 'Downloading at' })).toBeNull()
})

test('a download that stalls keeps the hero on it, at 0.0 Mbps', () => {
  const stalled: DownloadsView = {
    live: { bps: 0, parts: [] },
    list: [download(1, 'downloading'), download(2, 'downloading')],
  }
  show('live', table([wifi()], { combined: TWO }), [], { downloads: stalled })

  const h = screen.getByRole('region', { name: 'Downloading at' })
  expect(within(h).getByTestId('total')).toHaveTextContent(/^0\.0 Mbps$/)
  expect(h).toHaveTextContent('2 downloads, nothing arriving right now')
  expect(within(h).queryAllByTestId('segment')).toHaveLength(0)
})

test('Links shows the cables, Downloads the downloads; the hero and the activity stay on both', () => {
  const asked: Tab[] = []
  const t = table([wifi()], { combined: TWO })
  const onTab = (tab: Tab) => asked.push(tab)
  const { rerender } = show('live', t, [], { downloads: LIVE, onTab })

  expect(screen.getByRole('tab', { name: 'Links' })).toHaveAttribute('aria-selected', 'true')
  expect(screen.getByRole('tab', { name: 'Downloads' })).toHaveAttribute('aria-selected', 'false')
  expect(screen.getByRole('region', { name: 'Wi-Fi' })).toBeInTheDocument()
  expect(screen.queryByRole('textbox', { name: 'Download a file from its link' })).toBeNull()

  // A click asks for the tab; the page shows whichever it is given.
  fireEvent.click(screen.getByRole('tab', { name: 'Downloads' }))
  expect(asked).toEqual(['downloads'])

  rerender(page('live', t, [], { downloads: LIVE, onTab, tab: 'downloads' }))
  expect(screen.getByRole('tab', { name: 'Downloads' })).toHaveAttribute('aria-selected', 'true')
  expect(screen.getByRole('textbox', { name: 'Download a file from its link' })).toBeInTheDocument()
  expect(screen.getByRole('region', { name: 'file-2.iso' })).toBeInTheDocument()
  expect(screen.queryByRole('region', { name: 'Wi-Fi' })).toBeNull()
  expect(screen.getByRole('region', { name: 'Downloading at' })).toBeInTheDocument()
  expect(screen.getByRole('complementary', { name: 'Activity' })).toBeInTheDocument()

  fireEvent.click(screen.getByRole('tab', { name: 'Links' }))
  expect(asked).toEqual(['downloads', 'links'])
})

test('the engine never answered: says so, nothing else', () => {
  show('disconnected', null)
  expect(screen.getByText(/The engine is not answering/)).toHaveTextContent(
    'The engine is not answering. This page connects by itself once barrage runs.',
  )
  expect(screen.queryByRole('region', { name: 'Combined speed available' })).toBeNull()
})

test('the feed shows the newest line first, with its time', () => {
  const feed: FeedLine[] = [
    { at: new Date(2026, 9, 14, 13, 22, 41), text: 'Wi-Fi 192.0.2.10: speed 57.6 Mbps' },
    { at: new Date(2026, 9, 14, 13, 22, 9), text: '+ Wi-Fi 192.0.2.10' },
  ]
  show('live', table([wifi()]), feed)

  const activity = screen.getByRole('complementary', { name: 'Activity' })
  const items = within(activity).getAllByRole('listitem')
  expect(items.map((i) => [i.querySelector('time')!.textContent, i.querySelector('span')!.textContent])).toEqual([
    ['13:22:41', 'Wi-Fi 192.0.2.10: speed 57.6 Mbps'],
    ['13:22:09', '+ Wi-Fi 192.0.2.10'],
  ])
  expect(items[0].querySelector('time')).toHaveAttribute('datetime', feed[0].at.toISOString())
})

test('before the first table: connecting, no empty-state message', () => {
  show('connecting', null)
  expect(screen.getByTestId('light')).toHaveTextContent(/^connecting$/)
  expect(screen.queryByText(/No internet connections/)).toBeNull()
  expect(screen.queryByText(/not answering/)).toBeNull()
  expect(screen.queryByRole('region', { name: 'Combined speed available' })).toBeNull()
})

test('the connection light is a word for every status', () => {
  for (const [status, word] of [
    ['live', 'live'],
    ['connecting', 'connecting'],
    ['disconnected', 'disconnected, retrying'],
  ] as const) {
    const { unmount } = show(status, null)
    const light = screen.getByTestId('light')
    expect(light).toHaveTextContent(new RegExp(`^${word}$`))
    expect(light).toHaveClass('light', status)
    unmount()
  }
})
