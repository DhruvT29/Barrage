import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'

import App from './App'

// The page's own connection, replaced: these tests are about what App shows.
vi.mock('./api/useLive', () => ({
  useLive: () => ({
    status: 'live',
    table: {
      links: [
        {
          id: 1,
          name: 'Wi-Fi',
          address: '192.0.2.10',
          family: 'ipv4',
          state: 'up',
          rtt_ms: 40,
          down_bps: 57_600_000,
          public_ip: '203.0.113.7',
          metered: false,
        },
      ],
      shared: [],
      connections: [],
      combined: { bps: 57_600_000, parts: [{ name: 'Wi-Fi', bps: 57_600_000, shared_with: [] }], not_counted: [] },
    },
    feed: [],
    downloads: { live: { bps: 0, parts: [] }, list: [], used: [] },
  }),
}))

// Every test starts at the page's plain address, as `barrage` opens it.
beforeEach(() => window.history.replaceState(null, '', '/'))
afterEach(() => window.history.replaceState(null, '', '/'))

const tab = (name: string) => screen.getByRole('tab', { name })
const hero = () => screen.queryByRole('region', { name: 'Combined speed available' })

test('the app shows its name and its two tabs', () => {
  render(<App />)
  expect(screen.getByRole('heading', { name: 'BARRAGE' })).toBeInTheDocument()
  expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual(['Download manager', 'Device radar'])
})

test('the download manager is the tab shown, unless the address names another', () => {
  render(<App />)
  expect(tab('Download manager')).toHaveAttribute('aria-selected', 'true')
  expect(tab('Device radar')).toHaveAttribute('aria-selected', 'false')
  expect(hero()).toBeInTheDocument()
})

test('the radar tab survives a reload', () => {
  render(<App />)
  fireEvent.click(tab('Device radar'))
  expect(window.location.hash).toBe('#radar')
  expect(tab('Device radar')).toHaveAttribute('aria-selected', 'true')
  expect(hero()).toBeNull()

  // A reload: the page starts again from nothing but its address.
  cleanup()
  render(<App />)
  expect(tab('Device radar')).toHaveAttribute('aria-selected', 'true')
  expect(hero()).toBeNull()

  // Back on the download manager, the address is the plain one again.
  fireEvent.click(tab('Download manager'))
  expect(window.location.hash).toBe('')
  expect(window.location.pathname).toBe('/')
  expect(tab('Download manager')).toHaveAttribute('aria-selected', 'true')
  expect(hero()).toBeInTheDocument()
})

test('an address changed by hand, or by Back, moves the tab; the old #links still opens the radar', () => {
  render(<App />)
  act(() => {
    window.history.replaceState(null, '', '/#radar')
    window.dispatchEvent(new HashChangeEvent('hashchange'))
  })
  expect(tab('Device radar')).toHaveAttribute('aria-selected', 'true')

  act(() => {
    window.history.replaceState(null, '', '/')
    window.dispatchEvent(new HashChangeEvent('hashchange'))
  })
  expect(tab('Download manager')).toHaveAttribute('aria-selected', 'true')

  act(() => {
    window.history.replaceState(null, '', '/#links')
    window.dispatchEvent(new HashChangeEvent('hashchange'))
  })
  expect(tab('Device radar')).toHaveAttribute('aria-selected', 'true')
})

test('the dashboard shows what the live connection holds', () => {
  render(<App />)
  expect(screen.getByTestId('light')).toHaveAccessibleName('System: live')
  expect(within(hero()!).getByTestId('total')).toHaveTextContent('57.6 Mbps')
})
