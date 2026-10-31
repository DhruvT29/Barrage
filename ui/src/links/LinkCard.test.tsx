import { render, screen, within } from '@testing-library/react'
import type { ComponentProps } from 'react'
import { expect, test } from 'vitest'

import type { LinkState, LinkView } from '../api/types'
import type { Cable } from './cables'
import { LinkCard } from './LinkCard'

// Every link has its own id, as the engine's do: the card keys its addresses by it.
let nextId = 1

function link(address: string, fields: Partial<LinkView> = {}): LinkView {
  return {
    id: nextId++,
    name: 'Wi-Fi',
    address,
    family: address.includes(':') ? 'ipv6' : 'ipv4',
    state: 'up',
    rtt_ms: 40,
    down_bps: 57_600_000,
    public_ip: '203.0.113.7',
    metered: false,
    ...fields,
  }
}

function cable(fields: Partial<Cable> = {}): Cable {
  return {
    name: 'Wi-Fi',
    links: [link('192.0.2.10')],
    state: 'up',
    rttMs: 40,
    downBps: 57_600_000,
    sharedWith: [],
    metered: false,
    used: 0,
    ...fields,
  }
}

type Props = ComponentProps<typeof LinkCard>

/** An idle card: nothing downloads anywhere. */
function show(fields: Partial<Cable> = {}, props: Partial<Props> = {}) {
  return render(
    <LinkCard cable={cable(fields)} color="#22ff66" liveBps={null} liveTotal={0} fastest={false} series={[]} {...props} />,
  )
}

const card = (name = 'Wi-Fi') => screen.getByRole('region', { name })
const stat = (label: string) => within(card()).getByTestId(`stat-${label}`)

test('a card shows the name, the address summary and the state in words', () => {
  show({ links: [link('192.0.2.10'), link('2001:db8::10', { public_ip: '2001:db8::10' })] })

  const c = card()
  expect(within(c).getByRole('heading', { name: 'Wi-Fi' })).toBeInTheDocument()
  expect(within(c).getByTestId('summary')).toHaveTextContent('2 addresses · IPv4 + IPv6')
  expect(within(c).getByTestId('state')).toHaveTextContent(/^up$/)
  expect(stat('ping')).toHaveTextContent('40 ms')
})

test('idle: the big figure is the measured speed, shown once, and the bar says it carries nothing', () => {
  show({ links: [link('192.0.2.10'), link('2001:db8::10')] })

  const c = card()
  expect(within(c).getByTestId('figure-label')).toHaveTextContent(/^Measured$/)
  expect(within(c).getByTestId('speed')).toHaveTextContent('57.6 Mbps')
  expect(c.textContent!.match(/Mbps/g)).toHaveLength(1)
  expect(within(c).getByTestId('utilization')).toHaveTextContent('not carrying')
  expect(within(c).getByTestId('util-fill')).toHaveStyle({ width: '0%' })
  // Nothing downloads, so no share of it.
  expect(stat('share')).toHaveTextContent(/^Share-$/)
})

test('carrying: the big figure is the live downlink, the bar is live over measured, the share of the live total', () => {
  show({ downBps: 18_000_000 }, { liveBps: 11_000_000, liveTotal: 44_000_000 })

  const c = card()
  expect(within(c).getByTestId('figure-label')).toHaveTextContent(/^Live downlink$/)
  expect(within(c).getByTestId('speed')).toHaveTextContent(/^11\.0 Mbps$/)
  expect(within(c).getByTestId('utilization')).toHaveTextContent('61% / 18.0 Mbps')
  expect(parseFloat(within(c).getByTestId('util-fill').style.width)).toBeCloseTo(61.11, 1)
  expect(stat('share')).toHaveTextContent(/^Share25%$/)
  expect(c).toHaveClass('ml-carrying')
})

test('a cable idle while others download has a share of 0%', () => {
  show({}, { liveBps: null, liveTotal: 44_000_000 })
  expect(stat('share')).toHaveTextContent(/^Share0%$/)
  expect(card()).not.toHaveClass('ml-carrying')
})

test('a bar never runs past full, though the figure says what it is', () => {
  show({ downBps: 10_000_000 }, { liveBps: 12_500_000, liveTotal: 12_500_000 })
  expect(within(card()).getByTestId('utilization')).toHaveTextContent('125% / 10.0 Mbps')
  expect(within(card()).getByTestId('util-fill')).toHaveStyle({ width: '100%' })
})

test('unknown figures show as "-", never 0', () => {
  show({
    state: 'probing',
    rttMs: null,
    downBps: null,
    // No standalone 0 in the address, so a 0 anywhere on the card is a figure.
    links: [link('198.51.100.20', { state: 'probing', rtt_ms: null, down_bps: null, public_ip: null })],
  })

  const c = card()
  expect(within(c).getByTestId('state')).toHaveTextContent(/^probing$/)
  expect(within(c).getByTestId('speed')).toHaveTextContent(/^-$/)
  expect(stat('ping')).toHaveTextContent(/^Ping-$/)
  expect(stat('used')).toHaveTextContent(/^Used-$/)
  expect(within(c).getByTestId('public')).toHaveTextContent('public -')
  expect(c.textContent).not.toMatch(/\b0\b/)
})

test('each state shows its word, not only a colour', () => {
  const states: LinkState[] = ['up', 'probing', 'portal', 'degraded', 'down']
  for (const state of states) {
    const { unmount } = show({ state, rttMs: null, links: [link('192.0.2.10', { state })] })
    const c = card()
    expect(within(c).getByTestId('state')).toHaveTextContent(new RegExp(`^${state}$`))
    // The colour comes from the state's class, beside the word.
    expect(c).toHaveClass('ml-card', `ml-${state}`)
    unmount()
  }
})

test('the fastest cable wears the badge, the others do not', () => {
  const { unmount } = show({}, { fastest: true })
  expect(within(card()).getByTestId('fastest')).toHaveTextContent(/^Fastest$/)
  unmount()
  show()
  expect(within(card()).queryByTestId('fastest')).toBeNull()
})

test('the sparkline is drawn from the series given, and only a baseline before there is one', () => {
  const { container, unmount } = show({}, { series: [1, 4, 2, 8, 5] })
  expect(container.querySelector('[data-testid="spark-line"]')).not.toBeNull()
  unmount()
  const empty = show({}, { series: [0, 0, 0] })
  expect(empty.container.querySelector('[data-testid="spark-line"]')).toBeNull()
})

test('every address is listed with what the internet sees it as', () => {
  show({ links: [link('192.0.2.10'), link('2001:db8::10', { public_ip: '2001:db8::10' })] })

  const rows = within(card()).getAllByRole('listitem')
  const parts = rows.map((r) => [
    within(r).getByTestId('address').textContent,
    within(r).getByTestId('public').textContent,
  ])
  expect(parts).toEqual([
    ['192.0.2.10', 'public 203.0.113.7'],
    ['2001:db8::10', 'public same as address'],
  ])
})

test("an address whose state differs from the card's says its own", () => {
  show({
    links: [
      link('192.0.2.10', { state: 'down', rtt_ms: null, public_ip: null }),
      link('2001:db8::10', { public_ip: '2001:db8::10' }),
    ],
  })

  const rows = within(card()).getAllByRole('listitem')
  expect(within(rows[0]).getByTestId('address')).toHaveTextContent(/^192\.0\.2\.10 down$/)
  expect(within(rows[1]).getByTestId('address')).toHaveTextContent(/^2001:db8::10$/)
})

test('a shared cable names the cables it shares with', () => {
  show({ sharedWith: ['Ethernet 2', 'Ethernet 3'] })
  expect(within(card()).getByTestId('shared')).toHaveTextContent('shared with Ethernet 2 and Ethernet 3')
})

test('a cable on its own says nothing about sharing', () => {
  show()
  expect(within(card()).queryByTestId('shared')).toBeNull()
  expect(card().textContent).not.toMatch(/shared/)
})

test('a metered cable says so in words and a mark', () => {
  show({ metered: true })
  const metered = within(card()).getByTestId('metered')
  expect(metered).toHaveTextContent(/^metered$/)
  expect(metered.querySelector('svg')).not.toBeNull()
})

test('a cable Windows does not call metered has no mark and no word', () => {
  show()
  expect(within(card()).queryByTestId('metered')).toBeNull()
  expect(card().textContent).not.toMatch(/metered/i)
})

test("the data used shows once the cable has carried, in the page's sizes", () => {
  show({ used: 1_326_100_000 })
  expect(stat('used')).toHaveTextContent(/^Used1\.3 GB$/)
})

test('the icon follows the interface name', () => {
  const { unmount } = show({ name: 'Bluetooth Network' })
  expect(within(card('Bluetooth Network')).getByTestId('icon')).toHaveAttribute('data-kind', 'bluetooth')
  unmount()
  show({ name: 'Ethernet 2' })
  expect(within(card('Ethernet 2')).getByTestId('icon')).toHaveAttribute('data-kind', 'cable')
})
