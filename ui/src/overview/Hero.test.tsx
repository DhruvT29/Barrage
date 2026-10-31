import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, expect, test, vi } from 'vitest'

import { EMPTY_HISTORY, type SpeedHistory } from '../api/history'
import type { Status } from '../api/useLive'
import type { CombinedView, DownloadState, DownloadView, DownloadsView, LinkView, TableView } from '../api/types'
import { colorsByName } from '../palette'
import type { Tab } from '../tabs'
import { Hero } from './Hero'
import { forgetFirstTable } from './session'

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

let nextId = 1

function link(name: string, address: string, fields: Partial<LinkView> = {}): LinkView {
  return {
    id: nextId++,
    name,
    address,
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
    done: state === 'done' ? 1000 : 300,
    bps: 0,
    note: null,
    cables: [],
    ...fields,
  }
}

const wifi = () => link('Wi-Fi', '192.0.2.10', { down_bps: 57_600_000, rtt_ms: 12 })
const eth2 = () => link('Ethernet 2', '198.51.100.20', { down_bps: 162_500_000, rtt_ms: 6 })

const TWO: CombinedView = {
  bps: 220_100_000,
  parts: [
    { name: 'Wi-Fi', bps: 57_600_000, shared_with: [] },
    { name: 'Ethernet 2', bps: 162_500_000, shared_with: [] },
  ],
  not_counted: [],
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
  list: [download(1, 'done'), download(2, 'downloading', { length: 712_000_000, done: 0, bps: 8_000_000 })],
  used: [
    { cable: 'Wi-Fi', bytes: 1_000_000_000 },
    { cable: 'Ethernet 2', bytes: 500_000_000 },
  ],
}

type Props = {
  status?: Status
  table?: TableView | null
  downloads?: DownloadsView | null
  history?: SpeedHistory
  onAdd?: () => void
  onTab?: (tab: Tab) => void
}

function show({ status = 'live', table: t = null, downloads = null, history = EMPTY_HISTORY, onAdd = () => {}, onTab = () => {} }: Props) {
  const colors = colorsByName([...new Set((t?.links ?? []).map((l) => l.name))])
  return render(
    <Hero status={status} table={t} feed={[]} downloads={downloads} history={history} colors={colors} onAdd={onAdd} onTab={onTab} />,
  )
}

const available = () => screen.getByRole('region', { name: 'Combined speed available' })
const downloading = () => screen.getByRole('region', { name: 'Downloading at' })

test('idle: the combined speed available, a chip and a segment per independent connection', () => {
  show({ table: table([wifi(), eth2()], TWO) })
  const h = available()
  expect(within(h).getByTestId('total')).toHaveTextContent('220.1 Mbps')
  expect(h).toHaveTextContent('from 2 independent connections')
  expect(within(h).getByTestId('badge')).toHaveTextContent(/^Idle$/)
  const segments = within(h).getAllByTestId('segment')
  expect(segments.map((s) => parseFloat(s.style.width))).toEqual([expect.closeTo(26.17, 2), expect.closeTo(73.83, 2)])
  expect(within(h).getAllByTestId('part').map((p) => p.textContent)).toEqual(['Wi-Fi 57.6', 'Ethernet 2 162.5'])
  expect(within(h).getByTestId('sum')).toHaveTextContent('220.1 Mbps')
})

test('while anything downloads, the live download speed per cable, out of what is available', () => {
  show({ table: table([wifi(), eth2()], TWO), downloads: LIVE })
  const h = downloading()
  expect(within(h).getByTestId('total')).toHaveTextContent('189.9 Mbps')
  expect(h).toHaveTextContent('86% of 220.1 Mbps')
  expect(within(h).getByTestId('badge')).toHaveTextContent(/^Aggregate active$/)
  expect(within(h).getAllByTestId('part').map((p) => p.textContent)).toEqual(['Wi-Fi 48.2', 'Ethernet 2 141.7'])
  expect(within(h).getAllByTestId('segment').map((s) => parseFloat(s.style.width))).toEqual([
    expect.closeTo(25.38, 2),
    expect.closeTo(74.62, 2),
  ])
  expect(within(h).getByTestId('note')).toHaveTextContent(
    'What every download is fetching right now, added up by cable. With nothing downloading, this shows the combined speed available.',
  )
  expect(screen.queryByRole('region', { name: 'Combined speed available' })).toBeNull()
})

test('a download that stalls keeps the hero on it, at 0.0 Mbps', () => {
  const stalled: DownloadsView = { live: { bps: 0, parts: [] }, list: [download(1, 'downloading'), download(2, 'downloading')], used: [] }
  show({ table: table([wifi()], TWO), downloads: stalled })
  const h = downloading()
  expect(within(h).getByTestId('total')).toHaveTextContent(/^0\.0 Mbps$/)
  expect(within(h).queryAllByTestId('segment')).toHaveLength(0)
})

test('nothing measured yet: the total is "-", never 0, and no segments', () => {
  show({ table: table([link('Wi-Fi', '192.0.2.10', { state: 'probing' })], { bps: 0, parts: [], not_counted: [{ name: 'Wi-Fi', why: 'probing' }] }) })
  const h = available()
  expect(within(h).getByTestId('total')).toHaveTextContent(/^-$/)
  expect(h).toHaveTextContent('nothing measured yet')
  expect(within(h).queryAllByTestId('segment')).toHaveLength(0)
  expect(within(h).queryByTestId('sum')).toBeNull()
})

test('the note says what was counted once and what was not counted', () => {
  const combined: CombinedView = {
    bps: 57_600_000,
    parts: [{ name: 'Wi-Fi', bps: 57_600_000, shared_with: ['Ethernet 3'] }],
    not_counted: [
      { name: 'Ethernet 4', why: 'down' },
      { name: 'Ethernet 5', why: 'not measured yet' },
    ],
  }
  show({ table: table([wifi()], combined) })
  const note = within(available()).getByTestId('note')
  expect(note).toHaveTextContent("Ethernet 3 shares Wi-Fi's connection, so it is counted once with it.")
  expect(note).toHaveTextContent('Not counted: Ethernet 4 (down), Ethernet 5 (not measured yet).')
})

test('the cables left out are named, and the arrow opens the radar', () => {
  const asked: Tab[] = []
  const combined: CombinedView = { ...TWO, not_counted: [{ name: 'Bluetooth Network', why: 'down' }] }
  show({ table: table([wifi(), eth2(), link('Bluetooth Network', '192.0.2.200', { state: 'down' })], combined), onTab: (t) => asked.push(t) })
  expect(screen.getByTestId('not-counted')).toHaveTextContent('1 cable not counted: Bluetooth Network (down)')
  fireEvent.click(screen.getByRole('button', { name: 'Open the device radar' }))
  expect(asked).toEqual(['links'])
})

test('everything counted: no notice', () => {
  show({ table: table([wifi(), eth2()], TWO) })
  expect(screen.queryByTestId('not-counted')).toBeNull()
})

test('the stat tiles: links, average ping, active, fetched', () => {
  show({ table: table([wifi(), eth2(), link('Bluetooth Network', '192.0.2.200', { state: 'down', rtt_ms: null })], TWO), downloads: LIVE })
  const tile = (name: string) => screen.getByRole('group', { name })
  expect(tile('Links')).toHaveTextContent(/^Links2$/)
  expect(tile('Avg ping')).toHaveTextContent(/^Avg ping9 ms$/)
  expect(tile('Active')).toHaveTextContent(/^Active1$/)
  expect(tile('Fetched')).toHaveTextContent(/^Fetched1\.5 GB$/)
})

test('before anything is known, the tiles say "-"', () => {
  show({ status: 'connecting' })
  for (const name of ['Links', 'Avg ping', 'Active', 'Fetched']) {
    expect(screen.getByRole('group', { name })).toHaveTextContent(new RegExp(`^${name}-$`))
  }
})

test('the lead transfer: name, percent and time left; hidden when nothing downloads', () => {
  const { unmount } = show({ table: table([wifi()], TWO), downloads: LIVE })
  const lead = screen.getByRole('group', { name: 'Lead transfer' })
  expect(lead).toHaveTextContent('file-2.iso')
  expect(lead).toHaveTextContent('0%')
  expect(lead).toHaveTextContent('ETA 11m 52s')
  expect(within(lead).getByRole('progressbar')).toHaveAttribute('aria-valuenow', '0')
  unmount()

  show({ table: table([wifi()], TWO), downloads: { ...LIVE, list: [download(1, 'done')] } })
  expect(screen.queryByRole('group', { name: 'Lead transfer' })).toBeNull()
})

test('Add download asks for the window', () => {
  const onAdd = vi.fn()
  show({ table: table([wifi()], TWO), onAdd })
  fireEvent.click(screen.getByRole('button', { name: /Add download/ }))
  expect(onAdd).toHaveBeenCalledOnce()
})

test('Stop all is there only while something runs, and stops each running download', async () => {
  const asked: string[] = []
  vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
    asked.push(`${init.method} ${url} ${String(init.body)}`)
    return { ok: true, status: 200, json: async () => ({}) } as unknown as Response
  })
  const list = [download(1, 'downloading'), download(2, 'waiting'), download(3, 'done'), download(4, 'starting'), download(5, 'stopped')]
  const { unmount } = show({ table: table([wifi()], TWO), downloads: { ...LIVE, list } })

  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Stop all' }))
  })
  expect(asked).toEqual([
    'POST /api/downloads/1/stop {}',
    'POST /api/downloads/2/stop {}',
    'POST /api/downloads/4/stop {}',
  ])
  unmount()

  show({ table: table([wifi()], TWO), downloads: { ...LIVE, list: [download(1, 'done'), download(2, 'stopped')] } })
  expect(screen.queryByRole('button', { name: 'Stop all' })).toBeNull()
})

test('Stop all waits while the stops are on their way, and says a refusal in the engine\'s words', async () => {
  let answer: (r: Response) => void = () => {}
  vi.stubGlobal('fetch', () => new Promise<Response>((resolve) => (answer = resolve)))
  show({ table: table([wifi()], TWO), downloads: { ...LIVE, list: [download(7, 'downloading')] } })

  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Stop all' }))
  })
  expect(screen.getByRole('button', { name: 'Stop all' })).toBeDisabled()

  await act(async () => {
    answer({ ok: false, status: 409, json: async () => ({ error: 'download 7 is not running' }) } as unknown as Response)
  })
  expect(screen.getByRole('button', { name: 'Stop all' })).toBeEnabled()
  expect(screen.getByRole('alert')).toHaveTextContent('file-7.iso: download 7 is not running')
})

test('connected time counts from the first table the page got, and the badge says offline', () => {
  vi.useFakeTimers()
  forgetFirstTable()
  const t = table([wifi()], TWO)
  const { rerender } = show({ table: t })
  expect(screen.getByTestId('connected')).toHaveTextContent('Connected 00:00:00')
  act(() => vi.advanceTimersByTime(3 * 60_000 + 36_000))
  expect(screen.getByTestId('connected')).toHaveTextContent('Connected 00:03:36')

  const colors = colorsByName(['Wi-Fi'])
  rerender(
    <Hero status="disconnected" table={t} feed={[]} downloads={null} history={EMPTY_HISTORY} colors={colors} onAdd={() => {}} onTab={() => {}} />,
  )
  expect(screen.getByTestId('badge')).toHaveTextContent(/^Offline$/)
  expect(screen.getByTestId('connected')).toHaveTextContent('Retrying')
})

test('the engine never answered: says so, every figure unknown', () => {
  show({ status: 'disconnected' })
  const h = screen.getByRole('region', { name: 'Overview' })
  expect(h).toHaveTextContent('No figures until the engine answers. This page keeps trying by itself, and connects once barrage runs.')
  expect(within(h).getByTestId('total')).toHaveTextContent(/^-$/)
  expect(within(h).getByTestId('badge')).toHaveTextContent(/^Offline$/)
})

test('the chart draws a band per cable from the history, and its tooltip names each at that moment', () => {
  const history: SpeedHistory = {
    liveTotal: [0, 0, 0],
    measuredTotal: [80_000_000, 90_000_000, 100_000_000],
    live: { 'Wi-Fi': [0, 0, 0], 'Ethernet 2': [0, 0, 0] },
    measured: { 'Wi-Fi': [30_000_000, 40_000_000, 50_000_000], 'Ethernet 2': [50_000_000, 50_000_000, 50_000_000] },
  }
  show({ table: table([wifi(), eth2()], TWO), history })
  const chart = screen.getByRole('group', { name: /Measured speed, last minute/ })
  expect(chart.querySelectorAll('[data-testid="band"]')).toHaveLength(2)

  // The keyboard reaches every moment: the newest first.
  fireEvent.focus(chart)
  const tip = screen.getByTestId('chart-tip')
  expect(tip).toHaveTextContent('now')
  expect(tip).toHaveTextContent('Wi-Fi50.0 Mbps')
  expect(tip).toHaveTextContent('Ethernet 250.0 Mbps')
  fireEvent.keyDown(chart, { key: 'ArrowLeft' })
  expect(screen.getByTestId('chart-tip')).toHaveTextContent('1 s ago')
  expect(screen.getByTestId('chart-tip')).toHaveTextContent('Wi-Fi40.0 Mbps')
  fireEvent.blur(chart)
  expect(screen.queryByTestId('chart-tip')).toBeNull()
})

test('the chart follows the live series while downloading', () => {
  const history: SpeedHistory = {
    liveTotal: [10_000_000, 20_000_000],
    measuredTotal: [0, 0],
    live: { 'Wi-Fi': [10_000_000, 20_000_000] },
    measured: { 'Wi-Fi': [0, 0] },
  }
  show({ table: table([wifi()], TWO), downloads: LIVE, history })
  const chart = screen.getByRole('group', { name: /Download speed, last minute/ })
  fireEvent.focus(chart)
  expect(screen.getByTestId('chart-tip')).toHaveTextContent('Wi-Fi20.0 Mbps')
})

test('a measured 0 is a cable not measured yet: the tooltip says "-", never 0', () => {
  const history: SpeedHistory = {
    liveTotal: [0, 0],
    measuredTotal: [0, 40_000_000],
    live: { 'Wi-Fi': [0, 0] },
    measured: { 'Wi-Fi': [0, 40_000_000] },
  }
  show({ table: table([wifi()], TWO), history })
  const chart = screen.getByRole('group', { name: /Measured speed/ })
  fireEvent.focus(chart)
  fireEvent.keyDown(chart, { key: 'ArrowLeft' })
  const tip = screen.getByTestId('chart-tip')
  expect(tip).toHaveTextContent('Wi-Fi-')
  expect(tip).toHaveTextContent('Total-')
  expect(tip).not.toHaveTextContent('0.0 Mbps')
})

test('idle: a cable counted once with another (one shared connection) is not stacked again on the chart', () => {
  // Wi-Fi and Ethernet 2 reach the internet through one connection: the sum
  // counts it once, through Ethernet 2, so the chart's total must too.
  const shared: CombinedView = {
    bps: 162_500_000,
    parts: [{ name: 'Ethernet 2', bps: 162_500_000, shared_with: ['Wi-Fi'] }],
    not_counted: [],
  }
  const history: SpeedHistory = {
    liveTotal: [0, 0],
    measuredTotal: [162_500_000, 162_500_000],
    live: {},
    measured: { 'Wi-Fi': [57_600_000, 57_600_000], 'Ethernet 2': [162_500_000, 162_500_000] },
  }
  show({ table: table([wifi(), eth2()], shared), history })
  const chart = screen.getByRole('group', { name: /Measured speed, last minute/ })
  expect(chart.querySelectorAll('[data-testid="band"]')).toHaveLength(1)
  fireEvent.focus(chart)
  const tip = screen.getByTestId('chart-tip')
  expect(tip).toHaveTextContent('Total162.5 Mbps')
  expect(tip).not.toHaveTextContent('Wi-Fi')
})

test('Stop all says a refusal every stop got once, not once per download', async () => {
  vi.stubGlobal('fetch', async () => {
    throw new TypeError('Failed to fetch')
  })
  const list = [download(1, 'downloading'), download(2, 'waiting'), download(3, 'starting')]
  show({ table: table([wifi()], TWO), downloads: { ...LIVE, list } })
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Stop all' }))
  })
  expect(screen.getByRole('alert')).toHaveTextContent(/^3 downloads: the engine is not answering$/)
})

test('before the first table the chart does not claim a quiet minute: nothing has come from the engine', () => {
  const history: SpeedHistory = { liveTotal: [0, 0, 0], measuredTotal: [0, 0, 0], live: {}, measured: {} }
  show({ status: 'connecting', history })
  expect(screen.getByRole('group', { name: /last minute/ })).toHaveTextContent('Nothing from the engine yet')
  expect(screen.queryByText('Nothing measured in the last minute')).toBeNull()
})
