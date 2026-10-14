import { render, screen } from '@testing-library/react'
import { expect, test, vi } from 'vitest'

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
      combined: { bps: 57_600_000, parts: [{ name: 'Wi-Fi', bps: 57_600_000, shared_with: [] }], not_counted: [] },
    },
    feed: [],
  }),
}))

test('the app shows its name and only the tabs that are real', () => {
  render(<App />)
  expect(screen.getByRole('heading', { name: 'BARRAGE' })).toBeInTheDocument()
  expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual(['Links'])
})

test('the one tab is the selected one', () => {
  render(<App />)
  expect(screen.getByRole('tab', { name: 'Links' })).toHaveAttribute('aria-selected', 'true')
})

test('the dashboard shows what the live connection holds', () => {
  render(<App />)
  expect(screen.getByTestId('light')).toHaveTextContent('live')
  expect(screen.getByRole('region', { name: 'Wi-Fi' })).toBeInTheDocument()
})
