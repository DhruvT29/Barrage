import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { expect, test } from 'vitest'

import type { FeedLine, Status } from '../api/useLive'
import type { CombinedView, DownloadState, DownloadView, DownloadsView, LinkView, TableView } from '../api/types'
import type { Tab } from '../tabs'
import { Dashboard } from './Dashboard'

// The page as a whole: the header, the hero, the tab's view and the add
// window, put together. Each section's own behaviour is tested beside it
// (shell/, overview/, links/, downloads/).

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

const NOTHING: CombinedView = { bps: 0, parts: [], not_counted: [] }

function table(links: LinkView[], fields: Partial<TableView> = {}): TableView {
  return { links, shared: [], connections: [], combined: NOTHING, ...fields }
}

type More = { downloads: DownloadsView | null; tab: Tab; onTab: (tab: Tab) => void }

function page(status: Status, t: TableView | null, feed: FeedLine[] = [], more: Partial<More> = {}) {
  return <Dashboard status={status} table={t} feed={feed} downloads={null} tab="downloads" onTab={() => {}} {...more} />
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
  used: [],
}

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

const hero = () => screen.getByRole('region', { name: 'Combined speed available' })
/** The page's header: the bar that holds the name. */
const header = () => screen.getByRole('heading', { name: 'BARRAGE' }).closest('header')!

test('the header comes first, then the hero leads the download manager, then the cables and the queue', () => {
  show('live', table([wifi(), eth2()], { combined: TWO }))

  const h = hero()
  expect(header().compareDocumentPosition(h) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  expect(screen.getAllByRole('region')[0]).toBe(h)
  expect(within(h).getByTestId('total')).toHaveTextContent('220.1 Mbps')
  expect(screen.getByRole('region', { name: 'Mesh links' })).toBeInTheDocument()
  expect(screen.getByRole('region', { name: 'Transfer queue' })).toBeInTheDocument()
})

test("the header's throughput is the hero's figure", () => {
  show('live', table([wifi(), eth2()], { combined: TWO }), [], { downloads: LIVE })
  expect(within(screen.getByRole('region', { name: 'Downloading at' })).getByTestId('total')).toHaveTextContent(
    '189.9 Mbps',
  )
  expect(screen.getByTestId('throughput')).toHaveTextContent('189.9Mbps')
})

test('the radar tab shows the radar and no hero; a click on a tab asks for it', () => {
  const asked: Tab[] = []
  const t = table([wifi()], { combined: TWO })
  const onTab = (tab: Tab) => asked.push(tab)
  const { rerender } = show('live', t, [], { downloads: LIVE, onTab, tab: 'links' })

  expect(screen.getByRole('tab', { name: 'Device radar' })).toHaveAttribute('aria-selected', 'true')
  expect(screen.getByRole('tab', { name: 'Download manager' })).toHaveAttribute('aria-selected', 'false')
  expect(screen.queryByRole('region', { name: 'Downloading at' })).toBeNull()
  expect(screen.queryByRole('region', { name: 'Transfer queue' })).toBeNull()

  // A click asks for the tab; the page shows whichever it is given.
  fireEvent.click(screen.getByRole('tab', { name: 'Download manager' }))
  expect(asked).toEqual(['downloads'])

  rerender(page('live', t, [], { downloads: LIVE, onTab, tab: 'downloads' }))
  expect(screen.getByRole('tab', { name: 'Download manager' })).toHaveAttribute('aria-selected', 'true')
  expect(screen.getByRole('region', { name: 'Downloading at' })).toBeInTheDocument()
  expect(screen.getByRole('region', { name: 'Transfer queue' })).toBeInTheDocument()

  fireEvent.click(screen.getByRole('tab', { name: 'Device radar' }))
  expect(asked).toEqual(['downloads', 'links'])
})

test("the hero's notice opens the radar for the cables the sum left out", () => {
  const asked: Tab[] = []
  const combined: CombinedView = { ...TWO, not_counted: [{ name: 'Bluetooth Network', why: 'down' }] }
  show('live', table([wifi(), eth2()], { combined }), [], { onTab: (t) => asked.push(t) })
  fireEvent.click(screen.getByRole('button', { name: 'Open the device radar' }))
  expect(asked).toEqual(['links'])
})

test("the hero's Add download opens the add window; Escape closes it and gives focus back", () => {
  show('live', table([wifi()], { combined: TWO }))
  expect(screen.queryByRole('dialog')).toBeNull()

  const add = screen.getByRole('button', { name: /Add download/ })
  add.focus()
  fireEvent.click(add)
  const dialog = screen.getByRole('dialog', { name: 'Add a download' })
  expect(within(dialog).getByRole('textbox', { name: 'Link' })).toHaveFocus()

  act(() => {
    fireEvent.keyDown(document.activeElement!, { key: 'Escape' })
  })
  expect(screen.queryByRole('dialog')).toBeNull()
  expect(add).toHaveFocus()
})

test('disconnected: says so and still shows the last table, dimmed as old', () => {
  const t = table([wifi()], { combined: { bps: 57_600_000, parts: [TWO.parts[0]], not_counted: [] } })
  const { container, rerender } = show('disconnected', t)
  expect(screen.getByTestId('light')).toHaveAccessibleName('System: disconnected, retrying')
  expect(within(hero()).getByTestId('total')).toHaveTextContent('57.6 Mbps')
  expect(within(hero()).getByTestId('badge')).toHaveTextContent('Offline')
  expect(container.querySelector('.page')).toHaveClass('stale')
  // The header stays bright: it is what says the page is offline.
  expect(container.querySelector('.page')).not.toContainElement(header())

  rerender(page('live', t))
  expect(container.querySelector('.page')).not.toHaveClass('stale')
})

test('the engine never answered: says so, and nothing is dimmed, there being nothing old', () => {
  const { container } = show('disconnected', null)
  expect(screen.getByRole('region', { name: 'Overview' })).toHaveTextContent('No figures until the engine answers.')
  expect(screen.queryByRole('region', { name: 'Combined speed available' })).toBeNull()
  expect(container.querySelector('.page')).not.toHaveClass('stale')
})

test('before the first table: connecting, and every figure unknown', () => {
  show('connecting', null)
  expect(screen.getByTestId('light')).toHaveAccessibleName('System: connecting')
  expect(screen.queryByText(/until the engine answers/)).toBeNull()
  expect(screen.getByTestId('throughput')).toHaveTextContent(/^-$/)
  expect(screen.getByTestId('total')).toHaveTextContent(/^-$/)
})

test('each tab has one main heading, the name, and one main landmark holding its view', () => {
  const t = table([wifi(), eth2()], { combined: TWO })
  const { rerender } = show('live', t, [], { downloads: LIVE })
  for (const tab of ['downloads', 'links'] as const) {
    rerender(page('live', t, [], { downloads: LIVE, tab }))
    expect(screen.getAllByRole('heading', { level: 1 }).map((h) => h.textContent), tab).toEqual(['BARRAGE'])
    expect(screen.getAllByRole('main'), tab).toHaveLength(1)
  }
  expect(within(screen.getByRole('main')).getByRole('region', { name: 'Link detection' })).toBeInTheDocument()
})
