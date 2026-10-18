import { act, render, screen, within } from '@testing-library/react'
import { expect, test } from 'vitest'

import { Dashboard } from '../links/Dashboard'
import { messages } from './contract'
import type { LinkView, LiveMessage } from './types'
import { useLive } from './useLive'

// Every other test here builds its tables by hand. This one uses messages the
// engine really sent (contract.ts, written by the engine's own test), passed
// through the page's real socket handling as the text they arrive as, so a
// field renamed on one side and not the other fails here.

/** The engine's end of the socket. */
class EngineSocket {
  onmessage: ((event: { data: string }) => void) | null = null
  onclose: (() => void) | null = null
  close() {}

  send(text: string) {
    act(() => this.onmessage?.({ data: text }))
  }
}

function Page({ open }: { open: (url: string) => WebSocket }) {
  return <Dashboard {...useLive('ws://127.0.0.1:7700/api/live', open)} />
}

/** The cables, by name, in the order they first appear. */
const cableNames = (links: LinkView[]) => [...new Set(links.map((l) => l.name))]

test('the dashboard draws every message the engine really sends', () => {
  const all: LiveMessage[] = messages
  const socket = new EngineSocket()
  const open = () => socket as unknown as WebSocket
  const { container } = render(<Page open={open} />)

  for (const message of all) {
    socket.send(JSON.stringify(message))
    expect(container.querySelectorAll('section.cable')).toHaveLength(cableNames(message.table.links).length)
  }

  const last = all[all.length - 1]
  if (last.type !== 'change') throw new Error('the contract ends with a change')
  const { table } = last
  const { bps, parts, not_counted } = table.combined
  // The loops below prove nothing over an empty file.
  expect(table.links.length, 'links').toBeGreaterThan(0)
  expect(parts.length, 'combined parts').toBeGreaterThan(0)
  expect(not_counted.length, 'cables left out of the sum').toBeGreaterThan(0)

  // Every cable, with each of its addresses in order.
  for (const name of cableNames(table.links)) {
    const shown = within(screen.getByRole('region', { name })).getAllByTestId('address')
    const own = table.links.filter((l) => l.name === name)
    expect(shown).toHaveLength(own.length)
    own.forEach((link, i) => expect(shown[i]).toHaveTextContent(link.address))
  }

  // A cable shows its best address's word, and an address in another state
  // shows its own.
  const mixed = cableNames(table.links).find((name) => {
    const states = table.links.filter((l) => l.name === name).map((l) => l.state)
    return states.includes('degraded') && states.includes('portal')
  })
  expect(mixed, 'a cable with a degraded and a portal address').toBeDefined()
  const card = within(screen.getByRole('region', { name: mixed }))
  expect(card.getByTestId('state')).toHaveTextContent('degraded')
  expect(card.getByText('portal')).toBeInTheDocument()

  // The cables on one connection are named in the notice.
  expect(table.connections.length, 'a shared connection').toBeGreaterThan(0)
  const notice = screen.getByTestId('shared-notice')
  for (const name of table.connections.flat()) expect(notice).toHaveTextContent(name)

  // The combined speed, its parts, and what it left out and why.
  const hero = within(screen.getByRole('region', { name: 'Combined speed available' }))
  expect(hero.getByTestId('total')).toHaveTextContent(`${(bps / 1e6).toFixed(1)} Mbps`)
  expect(hero.getAllByTestId('part').map((p) => p.querySelector('b')?.textContent)).toEqual(parts.map((p) => p.name))
  const note = hero.getByTestId('note')
  for (const other of parts.flatMap((p) => p.shared_with)) expect(note).toHaveTextContent(other)
  for (const { name, why } of not_counted) expect(note).toHaveTextContent(`${name} (${why})`)

  // The change's lines, in the activity feed.
  expect(last.lines.length, 'the change has lines').toBeGreaterThan(0)
  for (const line of last.lines) expect(screen.getByText(line)).toBeInTheDocument()
})
