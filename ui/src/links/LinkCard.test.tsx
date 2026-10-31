import { render, screen, within } from '@testing-library/react'
import { expect, test } from 'vitest'

import type { LinkState, LinkView } from '../api/types'
import type { Cable } from './cables'
import { LinkCard } from './LinkCard'

function link(address: string, fields: Partial<LinkView> = {}): LinkView {
  return {
    id: 1,
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

const card = (name = 'Wi-Fi') => screen.getByRole('region', { name })

test('a card shows the name, the state with its rtt, and the speed once', () => {
  render(<LinkCard cable={cable()} fastestBps={57_600_000} />)

  const c = card()
  expect(within(c).getByRole('heading', { name: 'Wi-Fi' })).toBeInTheDocument()
  expect(within(c).getByTestId('state')).toHaveTextContent('up, 40 ms')
  expect(within(c).getByTestId('speed')).toHaveTextContent('57.6 Mbps')
  expect(c.textContent!.match(/Mbps/g)).toHaveLength(1)
})

test('the speed is shown once even when the cable has two addresses', () => {
  const two = cable({ links: [link('192.0.2.10'), link('2001:db8::10', { public_ip: '2001:db8::10' })] })
  render(<LinkCard cable={two} fastestBps={57_600_000} />)
  expect(card().textContent!.match(/Mbps/g)).toHaveLength(1)
})

test('unknown figures show as "-", never 0', () => {
  const unknown = cable({
    state: 'probing',
    rttMs: null,
    downBps: null,
    // No standalone 0 in the address, so a 0 anywhere on the card is a figure.
    links: [link('198.51.100.20', { state: 'probing', rtt_ms: null, down_bps: null, public_ip: null })],
  })
  render(<LinkCard cable={unknown} fastestBps={57_600_000} />)

  const c = card()
  expect(within(c).getByTestId('state')).toHaveTextContent(/^probing$/)
  expect(within(c).getByTestId('speed')).toHaveTextContent(/^-$/)
  expect(within(c).getByTestId('public')).toHaveTextContent('public -')
  expect(c.textContent).not.toMatch(/\b0\b/)
  expect(within(c).getByTestId('beam-fill')).toHaveStyle({ width: '0%' })
})

test('each state shows its word, not only a colour', () => {
  const states: LinkState[] = ['up', 'probing', 'portal', 'degraded', 'down']
  for (const state of states) {
    const { unmount } = render(
      <LinkCard cable={cable({ state, rttMs: null, links: [link('192.0.2.10', { state })] })} fastestBps={null} />,
    )
    const c = card()
    expect(within(c).getByTestId('state')).toHaveTextContent(new RegExp(`^${state}$`))
    // The colour and the shape come from the state's class, never the word alone or the colour alone.
    expect(c).toHaveClass('cable', state)
    unmount()
  }
})

test("the beam is the cable's speed as a share of the fastest cable's", () => {
  render(<LinkCard cable={cable({ downBps: 40_000_000 })} fastestBps={160_000_000} />)
  expect(within(card()).getByTestId('beam-fill')).toHaveStyle({ width: '25%' })
})

test('the fastest cable fills its beam', () => {
  render(<LinkCard cable={cable()} fastestBps={57_600_000} />)
  expect(within(card()).getByTestId('beam-fill')).toHaveStyle({ width: '100%' })
})

test('every address is listed with what the internet sees it as', () => {
  const two = cable({
    links: [link('192.0.2.10'), link('2001:db8::10', { public_ip: '2001:db8::10' })],
  })
  render(<LinkCard cable={two} fastestBps={57_600_000} />)

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
  const mixed = cable({
    links: [
      link('192.0.2.10', { state: 'down', rtt_ms: null, public_ip: null }),
      link('2001:db8::10', { public_ip: '2001:db8::10' }),
    ],
  })
  render(<LinkCard cable={mixed} fastestBps={57_600_000} />)

  const rows = within(card()).getAllByRole('listitem')
  expect(within(rows[0]).getByTestId('address')).toHaveTextContent(/^192\.0\.2\.10 down$/)
  expect(within(rows[1]).getByTestId('address')).toHaveTextContent(/^2001:db8::10$/)
})

test('a shared cable names the cables it shares with', () => {
  render(<LinkCard cable={cable({ sharedWith: ['Ethernet 2', 'Ethernet 3'] })} fastestBps={57_600_000} />)
  expect(within(card()).getByTestId('shared')).toHaveTextContent('shared with Ethernet 2 and Ethernet 3')
})

test('a cable on its own says nothing about sharing', () => {
  render(<LinkCard cable={cable()} fastestBps={57_600_000} />)
  expect(within(card()).queryByTestId('shared')).toBeNull()
  expect(card().textContent).not.toMatch(/shared/)
})

test('a metered cable says so beside its state, in words and a mark', () => {
  render(<LinkCard cable={cable({ metered: true })} fastestBps={57_600_000} />)
  const metered = within(card()).getByTestId('metered')
  expect(metered).toHaveTextContent(/^metered$/)
  expect(metered.querySelector('svg')).not.toBeNull()
  // Beside the state, not somewhere else on the card.
  expect(metered.closest('.state')).toBe(within(card()).getByTestId('state').closest('.state'))
})

test('a cable Windows does not call metered has no mark and no word', () => {
  render(<LinkCard cable={cable()} fastestBps={57_600_000} />)
  expect(within(card()).queryByTestId('metered')).toBeNull()
  expect(card().textContent).not.toMatch(/metered/)
})

test('the data used shows once the cable has carried, in the page\'s sizes', () => {
  render(<LinkCard cable={cable({ used: 1_326_100_000 })} fastestBps={57_600_000} />)
  expect(within(card()).getByTestId('used')).toHaveTextContent(/^used 1\.3 GB$/)
})

test('a cable that has carried nothing shows no data used', () => {
  render(<LinkCard cable={cable({ used: 0 })} fastestBps={57_600_000} />)
  expect(within(card()).queryByTestId('used')).toBeNull()
  expect(card().textContent).not.toMatch(/used/)
})
