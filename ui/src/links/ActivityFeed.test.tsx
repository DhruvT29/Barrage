import { render, screen, within } from '@testing-library/react'
import { expect, test } from 'vitest'

import type { FeedLine } from '../api/useLive'
import { ActivityFeed } from './ActivityFeed'

test('the engine\'s change lines, newest first, each with its local time', () => {
  const feed: FeedLine[] = [
    { at: new Date(2026, 9, 14, 13, 22, 41), text: 'Ethernet 2 198.51.100.12: up -> degraded' },
    { at: new Date(2026, 9, 14, 13, 22, 9), text: '+ Wi-Fi 192.0.2.10' },
  ]
  render(<ActivityFeed feed={feed} />)

  const activity = screen.getByRole('complementary', { name: 'Activity' })
  const items = within(activity).getAllByRole('listitem')
  expect(items.map((i) => [i.querySelector('time')!.textContent, i.querySelector('span')!.textContent])).toEqual([
    ['13:22:41', 'Ethernet 2 198.51.100.12: up -> degraded'],
    ['13:22:09', '+ Wi-Fi 192.0.2.10'],
  ])
  expect(items[0].querySelector('time')).toHaveAttribute('datetime', feed[0].at.toISOString())
})

test('a line that says a link went down is marked as such, one that adds a link as added', () => {
  const at = new Date(2026, 9, 14, 13, 0, 0)
  render(
    <ActivityFeed
      feed={[
        { at, text: 'Bluetooth Network 192.0.2.200: up -> down' },
        { at, text: '+ Wi-Fi 192.0.2.10' },
        { at, text: 'Wi-Fi 192.0.2.10: speed 57.6 Mbps' },
      ]}
    />,
  )
  const items = screen.getAllByRole('listitem')
  expect(items.map((i) => i.className)).toEqual(['rd-log-down', 'rd-log-add', 'rd-log-info'])
})

test('nothing yet: says so', () => {
  render(<ActivityFeed feed={[]} />)
  expect(screen.getByRole('complementary', { name: 'Activity' })).toHaveTextContent('Nothing has changed yet.')
})

test('an arrow in the engine\'s words is kept whole: a line never wraps between its "-" and ">"', () => {
  const feed: FeedLine[] = [{ at: new Date(2026, 9, 14, 13, 22, 41), text: 'Bluetooth Network 192.0.2.200: up -> down' }]
  render(<ActivityFeed feed={feed} />)
  const line = within(screen.getByRole('complementary', { name: 'Activity' })).getByRole('listitem')
  // Shown exactly as the engine wrote it.
  expect(line.querySelector('span')!.textContent).toBe('Bluetooth Network 192.0.2.200: up -> down')
  const arrows = line.querySelectorAll('.rd-nobreak')
  expect([...arrows].map((a) => a.textContent)).toEqual(['->'])
})
