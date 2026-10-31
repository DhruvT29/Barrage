import { render, screen, within } from '@testing-library/react'
import { expect, test } from 'vitest'

import { EMPTY_HISTORY } from '../api/history'
import type { DownloadsView, LinkView, TableView } from '../api/types'
import type { Status } from '../api/useLive'
import { colorsByName } from '../palette'
import type { PageProps } from './Dashboard'
import { MeshLinks } from './MeshLinks'

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

function page(status: Status, t: TableView | null, downloads: DownloadsView | null = null): PageProps {
  const names = [...new Set((t?.links ?? []).map((l) => l.name))]
  return { status, table: t, feed: [], downloads, history: EMPTY_HISTORY, colors: colorsByName(names) }
}

const THREE = () =>
  table([
    link('Wi-Fi', '192.0.2.24', { down_bps: 18_000_000 }),
    link('Ethernet', '192.0.2.81', { down_bps: 30_000_000 }),
    link('Ethernet 2', '198.51.100.12', { down_bps: 9_000_000 }),
    link('Ethernet 2', '2001:db8::5', { down_bps: 9_000_000 }),
  ])

test('a heading with the cable count, and a card per cable in table order', () => {
  render(<MeshLinks {...page('live', THREE())} />)

  const column = screen.getByRole('region', { name: 'Mesh links' })
  expect(within(column).getByRole('heading', { level: 2 })).toHaveTextContent('Mesh links 03')
  expect(column).toHaveTextContent('Plugged interfaces')
  const cards = within(column).getAllByRole('region')
  expect(cards.map((c) => c.getAttribute('aria-label'))).toEqual(['Wi-Fi', 'Ethernet', 'Ethernet 2'])
})

test('only the cable with the highest measured speed wears the fastest badge', () => {
  render(<MeshLinks {...page('live', THREE())} />)
  const badges = screen.getAllByTestId('fastest')
  expect(badges).toHaveLength(1)
  expect(badges[0].closest('section[aria-label]')).toHaveAttribute('aria-label', 'Ethernet')
})

test('a cable carrying a download shows its live speed and its share', () => {
  const downloads: DownloadsView = {
    live: {
      bps: 33_000_000,
      parts: [
        { name: 'Ethernet', bps: 22_000_000 },
        { name: 'Wi-Fi', bps: 11_000_000 },
      ],
    },
    list: [],
    used: [{ cable: 'Wi-Fi', bytes: 2_000_000_000 }],
  }
  render(<MeshLinks {...page('live', THREE(), downloads)} />)

  const wifi = screen.getByRole('region', { name: 'Wi-Fi' })
  expect(within(wifi).getByTestId('speed')).toHaveTextContent('11.0 Mbps')
  expect(within(wifi).getByTestId('stat-share')).toHaveTextContent('33%')
  expect(within(wifi).getByTestId('stat-used')).toHaveTextContent('2.0 GB')
  const phone = screen.getByRole('region', { name: 'Ethernet 2' })
  expect(within(phone).getByTestId('figure-label')).toHaveTextContent('Measured')
  expect(within(phone).getByTestId('stat-share')).toHaveTextContent('0%')
})

test('the shared notice names the cables on one connection and why', () => {
  const a = link('Wi-Fi', '192.0.2.10', { public_ip: '203.0.113.7' })
  const a2 = link('Wi-Fi', '192.0.2.11', { public_ip: '203.0.113.7' })
  const b = link('Ethernet 2', '198.51.100.20', { public_ip: '203.0.113.7' })
  render(<MeshLinks {...page('live', table([a, a2, b], { shared: [[a.id, a2.id, b.id]] }))} />)

  expect(screen.getByTestId('shared-notice')).toHaveTextContent(
    'One connection. Wi-Fi and Ethernet 2 reach the internet through one connection (203.0.113.7), so using both gains nothing.',
  )
})

test('no shared groups, no notice', () => {
  render(<MeshLinks {...page('live', THREE())} />)
  expect(screen.queryByTestId('shared-notice')).toBeNull()
})

test('no links: says so and how to get one', () => {
  render(<MeshLinks {...page('live', table([]))} />)
  expect(screen.getByText(/No internet connections\./)).toHaveTextContent(
    'No internet connections. Join a Wi-Fi network, or plug in a phone with USB tethering on.',
  )
  expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent('Mesh links 00')
})

test('the engine never answered: says so', () => {
  render(<MeshLinks {...page('disconnected', null)} />)
  expect(screen.getByText(/The engine is not answering/)).toHaveTextContent(
    'The engine is not answering. This page connects by itself once barrage runs.',
  )
  expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent(/^Mesh links -$/)
})

test('before the first table: no empty-state message', () => {
  render(<MeshLinks {...page('connecting', null)} />)
  expect(screen.queryByText(/No internet connections/)).toBeNull()
  expect(screen.queryByText(/not answering/)).toBeNull()
})
