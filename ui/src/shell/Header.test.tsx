import { fireEvent, render, screen } from '@testing-library/react'
import { expect, test } from 'vitest'

import { EMPTY_HISTORY } from '../api/history'
import type { Status } from '../api/useLive'
import type { CombinedView, DownloadsView, LinkView, TableView } from '../api/types'
import type { Tab } from '../tabs'
import { Header } from './Header'

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

const TWO: CombinedView = {
  bps: 57_000_000,
  parts: [
    { name: 'Ethernet', bps: 30_000_000, shared_with: [] },
    { name: 'Wi-Fi', bps: 27_000_000, shared_with: [] },
  ],
  not_counted: [],
}

const TABLE: TableView = {
  links: [link('Wi-Fi'), link('Ethernet', { state: 'degraded' }), link('Bluetooth Network', { state: 'down' })],
  shared: [],
  connections: [],
  combined: TWO,
}

function show(status: Status, table: TableView | null, downloads: DownloadsView | null = null, tab: Tab = 'downloads', onTab: (t: Tab) => void = () => {}) {
  return render(
    <Header status={status} table={table} feed={[]} downloads={downloads} history={EMPTY_HISTORY} colors={{}} tab={tab} onTab={onTab} />,
  )
}

test('the name, and the two tabs in order, the shown one selected', () => {
  show('live', TABLE)
  expect(screen.getByRole('heading', { name: 'BARRAGE' })).toBeInTheDocument()
  expect(screen.getByRole('tablist')).toBeInTheDocument()
  expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual(['Download manager', 'Device radar'])
  expect(screen.getByRole('tab', { name: 'Download manager' })).toHaveAttribute('aria-selected', 'true')
  expect(screen.getByRole('tab', { name: 'Device radar' })).toHaveAttribute('aria-selected', 'false')
})

test('a click asks for the tab; arrow keys move between them', () => {
  const asked: Tab[] = []
  show('live', TABLE, null, 'downloads', (t) => asked.push(t))
  fireEvent.click(screen.getByRole('tab', { name: 'Device radar' }))
  fireEvent.keyDown(screen.getByRole('tab', { name: 'Download manager' }), { key: 'ArrowRight' })
  fireEvent.keyDown(screen.getByRole('tab', { name: 'Download manager' }), { key: 'ArrowLeft' })
  expect(asked).toEqual(['links', 'links', 'links'])
})

test('only the selected tab is in the tab order', () => {
  show('live', TABLE, null, 'links')
  expect(screen.getByRole('tab', { name: 'Device radar' })).toHaveAttribute('tabindex', '0')
  expect(screen.getByRole('tab', { name: 'Download manager' })).toHaveAttribute('tabindex', '-1')
})

test('throughput is the combined speed available while nothing downloads, and links count the usable cables', () => {
  show('live', TABLE, { live: { bps: 0, parts: [] }, list: [], used: [] })
  expect(screen.getByTestId('throughput')).toHaveTextContent(/^57\.0Mbps$/)
  expect(screen.getByTestId('linked')).toHaveTextContent(/^02Linked$/)
})

test('throughput is the live download speed while anything downloads', () => {
  const downloads: DownloadsView = {
    live: { bps: 12_300_000, parts: [{ name: 'Wi-Fi', bps: 12_300_000 }] },
    list: [{ id: 1, url: 'https://example.com/a.iso', name: 'a.iso', state: 'downloading', length: null, done: 0, bps: 12_300_000, note: null, cables: [] }],
    used: [],
  }
  show('live', TABLE, downloads)
  expect(screen.getByTestId('throughput')).toHaveTextContent(/^12\.3Mbps$/)
})

test('before the first table, or with nothing measured, the figures are "-", never 0', () => {
  show('connecting', null)
  expect(screen.getByTestId('throughput')).toHaveTextContent(/^-$/)
  expect(screen.getByTestId('linked')).toHaveTextContent(/^-$/)
})

test('the connection to the engine is a word for every status', () => {
  for (const [status, word, spoken] of [
    ['live', 'Live', 'live'],
    ['connecting', 'Connecting', 'connecting'],
    ['disconnected', 'Offline', 'disconnected, retrying'],
  ] as const) {
    const { unmount } = show(status, null)
    const light = screen.getByTestId('light')
    expect(light).toHaveAttribute('role', 'status')
    expect(light).toHaveClass(status)
    expect(light.querySelector('.sh-word')).toHaveTextContent(new RegExp(`^${word}$`))
    expect(light).toHaveAccessibleName(`System: ${spoken}`)
    unmount()
  }
})
