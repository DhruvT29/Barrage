import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
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
        },
      ],
      shared: [],
      connections: [],
      combined: { bps: 57_600_000, parts: [{ name: 'Wi-Fi', bps: 57_600_000, shared_with: [] }], not_counted: [] },
    },
    feed: [],
    downloads: { live: { bps: 0, parts: [] }, list: [] },
  }),
}))

// Every test starts at the page's plain address, as `barrage` opens it.
beforeEach(() => window.history.replaceState(null, '', '/'))
afterEach(() => window.history.replaceState(null, '', '/'))

const tab = (name: string) => screen.getByRole('tab', { name })
const field = () => screen.queryByRole('textbox', { name: 'Download a file from its link' })

test('the app shows its name and its two tabs', () => {
  render(<App />)
  expect(screen.getByRole('heading', { name: 'BARRAGE' })).toBeInTheDocument()
  expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual(['Links', 'Downloads'])
})

test('Links is the tab shown, unless the address names another', () => {
  render(<App />)
  expect(tab('Links')).toHaveAttribute('aria-selected', 'true')
  expect(tab('Downloads')).toHaveAttribute('aria-selected', 'false')
  expect(screen.getByRole('region', { name: 'Wi-Fi' })).toBeInTheDocument()
})

test('the Downloads tab survives a reload', () => {
  render(<App />)
  fireEvent.click(tab('Downloads'))
  expect(window.location.hash).toBe('#downloads')
  expect(tab('Downloads')).toHaveAttribute('aria-selected', 'true')
  expect(field()).toBeInTheDocument()

  // A reload: the page starts again from nothing but its address.
  cleanup()
  render(<App />)
  expect(tab('Downloads')).toHaveAttribute('aria-selected', 'true')
  expect(field()).toBeInTheDocument()

  // Back on Links, the address is the plain one again.
  fireEvent.click(tab('Links'))
  expect(window.location.hash).toBe('')
  expect(window.location.pathname).toBe('/')
  expect(tab('Links')).toHaveAttribute('aria-selected', 'true')
  expect(field()).toBeNull()
})

test('an address changed by hand, or by Back, moves the tab', () => {
  render(<App />)
  act(() => {
    window.history.replaceState(null, '', '/#downloads')
    window.dispatchEvent(new HashChangeEvent('hashchange'))
  })
  expect(tab('Downloads')).toHaveAttribute('aria-selected', 'true')

  act(() => {
    window.history.replaceState(null, '', '/#links')
    window.dispatchEvent(new HashChangeEvent('hashchange'))
  })
  expect(tab('Links')).toHaveAttribute('aria-selected', 'true')
})

test('the dashboard shows what the live connection holds', () => {
  render(<App />)
  expect(screen.getByTestId('light')).toHaveTextContent('live')
  expect(screen.getByRole('region', { name: 'Wi-Fi' })).toBeInTheDocument()
})
