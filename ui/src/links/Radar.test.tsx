import { fireEvent, render, screen, within } from '@testing-library/react'
import { expect, test } from 'vitest'

import { EMPTY_HISTORY } from '../api/history'
import type { DownloadsView, LinkView, TableView } from '../api/types'
import type { FeedLine, Status } from '../api/useLive'
import { colorsByName } from '../palette'
import type { PageProps } from './Dashboard'
import { RadarView } from './Radar'

let nextId = 1

function link(name: string, address: string, fields: Partial<LinkView> = {}): LinkView {
  return {
    id: nextId++,
    name,
    address,
    family: address.includes(':') ? 'ipv6' : 'ipv4',
    state: 'up',
    rtt_ms: 12,
    down_bps: null,
    public_ip: null,
    metered: false,
    ...fields,
  }
}

function table(links: LinkView[], fields: Partial<TableView> = {}): TableView {
  return { links, shared: [], connections: [], combined: { bps: 0, parts: [], not_counted: [] }, ...fields }
}

function page(
  status: Status,
  t: TableView | null,
  downloads: DownloadsView | null = null,
  feed: FeedLine[] = [],
): PageProps {
  const names = [...new Set((t?.links ?? []).map((l) => l.name))]
  return { status, table: t, feed, downloads, history: EMPTY_HISTORY, colors: colorsByName(names) }
}

const wifi = () => link('Wi-Fi', '192.0.2.24', { rtt_ms: 12, down_bps: 18_000_000, public_ip: '203.0.113.7' })
const eth = () => link('Ethernet', '192.0.2.81', { rtt_ms: 4, down_bps: 30_000_000, public_ip: '203.0.113.9' })
const phone = () => link('Ethernet 2', '198.51.100.12', { rtt_ms: 38, down_bps: 9_000_000, metered: true })
const phone6 = () => link('Ethernet 2', '2001:db8::5', { rtt_ms: 41, down_bps: 9_000_000, metered: true })
const bt = () => link('Bluetooth Network', '192.0.2.200', { state: 'down', rtt_ms: null })

const FOUR = () => table([wifi(), eth(), phone(), phone6(), bt()])

/** Wi-Fi and Ethernet carry; the phone and Bluetooth do not. */
const LIVE: DownloadsView = {
  live: {
    bps: 33_000_000,
    parts: [
      { name: 'Ethernet', bps: 22_000_000 },
      { name: 'Wi-Fi', bps: 11_000_000 },
    ],
  },
  list: [],
  used: [
    { cable: 'Ethernet', bytes: 3_000_000_000 },
    { cable: 'Wi-Fi', bytes: 1_000_000_000 },
  ],
}

const radar = () => screen.getByRole('group', { name: 'Radar' })
const roster = () => screen.getByRole('region', { name: 'Contact roster' })
const detail = () => screen.getByRole('region', { name: 'Selected cable' })
const node = (name: string) => within(radar()).getByRole('button', { name: new RegExp(`^${name}:`) })
const nodeNames = () => within(radar()).queryAllByRole('button').map((b) => b.getAttribute('aria-label')!.split(':')[0])
const rosterNames = () =>
  within(roster()).queryAllByRole('button').map((b) => b.getAttribute('aria-label')!.split(':')[0])

test('one node per cable, each named with its state in words', () => {
  render(<RadarView {...page('live', FOUR(), LIVE)} />)

  expect(nodeNames()).toEqual(['Wi-Fi', 'Ethernet', 'Ethernet 2', 'Bluetooth Network'])
  expect(node('Wi-Fi')).toHaveAttribute('aria-label', 'Wi-Fi: up, carrying, 12 ms, 18.0 Mbps')
  expect(node('Bluetooth Network')).toHaveAttribute('aria-label', 'Bluetooth Network: down, no round trip yet, not measured')
  expect(radar()).toHaveTextContent('THIS PC')
})

test('the first cable is selected by default', () => {
  render(<RadarView {...page('live', FOUR(), LIVE)} />)
  expect(node('Wi-Fi')).toHaveAttribute('aria-pressed', 'true')
  expect(node('Ethernet')).toHaveAttribute('aria-pressed', 'false')
  expect(within(detail()).getByRole('heading', { level: 2 })).toHaveTextContent('Wi-Fi')
})

test('a click on a node selects it', () => {
  render(<RadarView {...page('live', FOUR(), LIVE)} />)
  fireEvent.click(node('Ethernet 2'))
  expect(node('Ethernet 2')).toHaveAttribute('aria-pressed', 'true')
  expect(node('Wi-Fi')).toHaveAttribute('aria-pressed', 'false')
  expect(within(detail()).getByRole('heading', { level: 2 })).toHaveTextContent('Ethernet 2')
})

test('Enter or Space on a focused node selects it', () => {
  render(<RadarView {...page('live', FOUR(), LIVE)} />)
  const n = node('Ethernet')
  expect(n).toHaveAttribute('tabindex', '0')
  fireEvent.keyDown(n, { key: 'Enter' })
  expect(within(detail()).getByRole('heading', { level: 2 })).toHaveTextContent(/^Ethernet$/)
  fireEvent.keyDown(node('Bluetooth Network'), { key: ' ' })
  expect(within(detail()).getByRole('heading', { level: 2 })).toHaveTextContent('Bluetooth Network')
  // Other keys do nothing.
  fireEvent.keyDown(node('Wi-Fi'), { key: 'a' })
  expect(within(detail()).getByRole('heading', { level: 2 })).toHaveTextContent('Bluetooth Network')
})

test('the selection is kept by name when the table changes, and falls back to the first when its cable goes', () => {
  const { rerender } = render(<RadarView {...page('live', FOUR(), LIVE)} />)
  fireEvent.click(node('Ethernet'))

  // A new cable appears ahead of it: still Ethernet.
  rerender(<RadarView {...page('live', table([link('Ethernet 9', '198.51.100.90'), wifi(), eth()]), LIVE)} />)
  expect(node('Ethernet')).toHaveAttribute('aria-pressed', 'true')
  expect(within(detail()).getByRole('heading', { level: 2 })).toHaveTextContent(/^Ethernet$/)

  // Ethernet goes: the first cable.
  rerender(<RadarView {...page('live', table([link('Ethernet 9', '198.51.100.90'), wifi()]), LIVE)} />)
  expect(within(detail()).getByRole('heading', { level: 2 })).toHaveTextContent('Ethernet 9')
})

test('the filter shows only the carrying cables, or only the idle ones, on the radar and the roster', () => {
  render(<RadarView {...page('live', FOUR(), LIVE)} />)
  const filter = screen.getByRole('group', { name: 'Show' })
  expect(within(filter).getByRole('button', { name: 'All' })).toHaveAttribute('aria-pressed', 'true')

  fireEvent.click(within(filter).getByRole('button', { name: 'Carrying' }))
  expect(within(filter).getByRole('button', { name: 'Carrying' })).toHaveAttribute('aria-pressed', 'true')
  expect(within(filter).getByRole('button', { name: 'All' })).toHaveAttribute('aria-pressed', 'false')
  expect(nodeNames()).toEqual(['Wi-Fi', 'Ethernet'])
  expect(rosterNames()).toEqual(['Wi-Fi', 'Ethernet'])

  fireEvent.click(within(filter).getByRole('button', { name: 'Idle' }))
  expect(nodeNames()).toEqual(['Ethernet 2', 'Bluetooth Network'])
  expect(rosterNames()).toEqual(['Ethernet 2', 'Bluetooth Network'])
  // Wi-Fi was selected and is hidden now: the first one shown is.
  expect(within(detail()).getByRole('heading', { level: 2 })).toHaveTextContent('Ethernet 2')

  fireEvent.click(within(filter).getByRole('button', { name: 'All' }))
  expect(nodeNames()).toHaveLength(4)
})

test('a filter that matches nothing says so', () => {
  render(<RadarView {...page('live', FOUR(), null)} />)
  fireEvent.click(within(screen.getByRole('group', { name: 'Show' })).getByRole('button', { name: 'Carrying' }))
  expect(nodeNames()).toEqual([])
  expect(screen.getAllByText('No cable is carrying a download right now.').length).toBeGreaterThan(0)
})

test('the roster counts the cables shown and those not usable, and a row selects', () => {
  render(<RadarView {...page('live', FOUR(), LIVE)} />)
  expect(within(roster()).getByRole('heading', { level: 2 })).toHaveTextContent('Detected 04')
  expect(roster()).toHaveTextContent('1 not usable')

  const row = within(roster()).getByRole('button', { name: /^Ethernet 2:/ })
  expect(row).toHaveTextContent('Ethernet 2')
  expect(row).toHaveTextContent('up')
  expect(row).toHaveTextContent('38 ms')
  expect(row).toHaveTextContent('9.0 Mbps')
  fireEvent.click(row)
  expect(row).toHaveAttribute('aria-pressed', 'true')
  expect(within(detail()).getByRole('heading', { level: 2 })).toHaveTextContent('Ethernet 2')
})

test('the states table counts the cables in each state, in words', () => {
  render(<RadarView {...page('live', FOUR(), LIVE)} />)
  const states = screen.getByRole('table', { name: 'States' })
  const rows = within(states)
    .getAllByRole('row')
    .map((r) => r.textContent)
  expect(rows).toEqual(['up3', 'degraded0', 'probing0', 'portal0', 'down1'])
})

test('the detail of a carrying cable: its live speed against its capacity, and its share', () => {
  render(<RadarView {...page('live', FOUR(), LIVE)} />)
  const d = detail()
  expect(within(d).getByTestId('badge')).toHaveTextContent(/^up · carrying$/)
  expect(within(d).getByTestId('summary')).toHaveTextContent('1 address · IPv4')
  expect(within(d).getByTestId('actual')).toHaveTextContent('11.0 Mbps')
  expect(within(d).getByTestId('capacity')).toHaveTextContent('18.0 Mbps')
  expect(within(d).getByTestId('ring-share')).toHaveTextContent('33%')
  expect(within(d).getByTestId('ring-latency')).toHaveTextContent('12 ms')
  expect(within(d).getByTestId('ring-used')).toHaveTextContent('1.0 GB')
  expect(within(d).getByTestId('ring-metered')).toHaveTextContent('no')
  expect(within(d).queryByText(/Idle/)).toBeNull()
})

test('the detail of an idle, metered cable with two addresses', () => {
  render(<RadarView {...page('live', FOUR(), LIVE)} />)
  fireEvent.click(node('Ethernet 2'))
  const d = detail()
  expect(within(d).getByTestId('badge')).toHaveTextContent(/^up$/)
  expect(within(d).getByTestId('summary')).toHaveTextContent('2 addresses · IPv4 + IPv6')
  expect(within(d).getByText('Idle: not carrying a download right now.')).toBeInTheDocument()
  expect(within(d).getByTestId('ring-share')).toHaveTextContent('0%')
  expect(within(d).getByTestId('ring-used')).toHaveTextContent('-')
  expect(within(d).getByTestId('ring-metered')).toHaveTextContent('yes')
  const facts = within(d).getByTestId('facts')
  expect(facts).toHaveTextContent('198.51.100.12')
  expect(facts).toHaveTextContent('2001:db8::5')
  expect(facts).toHaveTextContent('IPv4 + IPv6')
})

test('the detail of a cable never probed: unknown is "-", never 0', () => {
  render(<RadarView {...page('live', FOUR(), null)} />)
  fireEvent.click(node('Bluetooth Network'))
  const d = detail()
  expect(within(d).getByTestId('badge')).toHaveTextContent(/^down$/)
  expect(within(d).getByTestId('actual')).toHaveTextContent(/^-$/)
  expect(within(d).getByTestId('capacity')).toHaveTextContent(/^-$/)
  expect(within(d).getByTestId('ring-share')).toHaveTextContent('-')
  expect(within(d).getByTestId('ring-latency')).toHaveTextContent('-')
})

test('the header says watching while live and offline while disconnected', () => {
  const { rerender } = render(<RadarView {...page('live', FOUR())} />)
  expect(screen.getByTestId('watch')).toHaveTextContent(/^watching$/)
  rerender(<RadarView {...page('disconnected', FOUR())} />)
  expect(screen.getByTestId('watch')).toHaveTextContent(/^offline$/)
})

test('the activity log shows the newest line first, with its time', () => {
  const feed: FeedLine[] = [
    { at: new Date(2026, 9, 14, 13, 22, 41), text: 'Wi-Fi 192.0.2.10: speed 57.6 Mbps' },
    { at: new Date(2026, 9, 14, 13, 22, 9), text: '+ Wi-Fi 192.0.2.10' },
  ]
  render(<RadarView {...page('live', FOUR(), null, feed)} />)
  const activity = screen.getByRole('complementary', { name: 'Activity' })
  const items = within(activity).getAllByRole('listitem')
  expect(items.map((i) => [i.querySelector('time')!.textContent, i.querySelector('span')!.textContent])).toEqual([
    ['13:22:41', 'Wi-Fi 192.0.2.10: speed 57.6 Mbps'],
    ['13:22:09', '+ Wi-Fi 192.0.2.10'],
  ])
})

test('no links: says so and how to get one', () => {
  render(<RadarView {...page('live', table([]))} />)
  expect(screen.getAllByText(/No internet connections\./)[0]).toHaveTextContent(
    'No internet connections. Join a Wi-Fi network, or plug in a phone with USB tethering on.',
  )
  expect(nodeNames()).toEqual([])
})

test('the engine never answered: says so', () => {
  render(<RadarView {...page('disconnected', null)} />)
  expect(screen.getAllByText(/The engine is not answering/)[0]).toHaveTextContent(
    'The engine is not answering. This page connects by itself once barrage runs.',
  )
})

test('before the first table: no empty-state message', () => {
  render(<RadarView {...page('connecting', null)} />)
  expect(screen.queryByText(/No internet connections/)).toBeNull()
  expect(screen.queryByText(/not answering/)).toBeNull()
})

test('before the first table, every count is unknown ("-", never 0), and the inspector claims no cable to select', () => {
  render(<RadarView {...page('connecting', null)} />)
  expect(screen.getByTestId('in-range')).toHaveTextContent(/^-/)
  const states = screen.getByRole('table', { name: 'States' })
  for (const row of within(states).getAllByRole('row')) expect(within(row).getByRole('cell')).toHaveTextContent(/^-$/)
  expect(within(roster()).getByRole('heading', { level: 2 })).toHaveTextContent(/^Detected -$/)
  expect(detail()).not.toHaveTextContent(/Select a cable/)
  expect(detail()).toHaveTextContent("Waiting for the engine's first table.")
})

test('with a table, the counts are real, a 0 included', () => {
  render(<RadarView {...page('live', table([]))} />)
  expect(screen.getByTestId('in-range')).toHaveTextContent(/^00/)
  expect(within(roster()).getByRole('heading', { level: 2 })).toHaveTextContent(/^Detected 00$/)
  expect(detail()).toHaveTextContent('Nothing to inspect.')
})
