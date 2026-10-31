import { act, render, screen, within } from '@testing-library/react'
import { expect, test } from 'vitest'

import { formatBytes, percent } from '../downloads/format'
import { Combined } from '../links/Combined'
import { Dashboard } from '../links/Dashboard'
import type { Tab } from '../tabs'
import { messages } from './contract'
import type { DownloadsView, LinkView, LiveMessage } from './types'
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

function Page({ open, tab }: { open: (url: string) => WebSocket; tab: Tab }) {
  return <Dashboard {...useLive('ws://127.0.0.1:7700/api/live', open)} tab={tab} onTab={() => {}} />
}

/** The cables, by name, in the order they first appear. */
const cableNames = (links: LinkView[]) => [...new Set(links.map((l) => l.name))]

/** The last downloads the engine sent. */
function lastDownloads(all: LiveMessage[]): DownloadsView {
  const sent = all.flatMap((m) => (m.type === 'downloads' ? [m.downloads] : []))
  expect(sent.length, 'a downloads message').toBeGreaterThan(0)
  return sent[sent.length - 1]
}

test('the dashboard draws every message the engine really sends', () => {
  const all: LiveMessage[] = messages
  const socket = new EngineSocket()
  const open = () => socket as unknown as WebSocket
  const { container } = render(<Page open={open} tab="links" />)

  // A downloads message carries no table: the cards stay as the last table drew them.
  let drawn: LinkView[] = []
  for (const message of all) {
    socket.send(JSON.stringify(message))
    if (message.type !== 'downloads') drawn = message.table.links
    expect(container.querySelectorAll('section.cable')).toHaveLength(cableNames(drawn).length)
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

  // A download is running, so the hero is its live speed, by cable.
  const { live } = lastDownloads(all)
  expect(live.parts.length, 'cables carrying data').toBeGreaterThan(0)
  const running = within(screen.getByRole('region', { name: 'Downloading at' }))
  expect(running.getByTestId('total')).toHaveTextContent(`${(live.bps / 1e6).toFixed(1)} Mbps`)
  expect(running.getAllByTestId('part').map((p) => p.querySelector('b')?.textContent)).toEqual(
    live.parts.map((p) => p.name),
  )

  // With nothing downloading, the same place is the combined speed: its
  // parts, and what it left out and why.
  const { container: alone } = render(<Combined combined={table.combined} downloads={null} />)
  const hero = within(within(alone).getByRole('region', { name: 'Combined speed available' }))
  expect(hero.getByTestId('total')).toHaveTextContent(`${(bps / 1e6).toFixed(1)} Mbps`)
  expect(hero.getAllByTestId('part').map((p) => p.querySelector('b')?.textContent)).toEqual(parts.map((p) => p.name))
  const note = hero.getByTestId('note')
  for (const other of parts.flatMap((p) => p.shared_with)) expect(note).toHaveTextContent(other)
  for (const { name, why } of not_counted) expect(note).toHaveTextContent(`${name} (${why})`)

  // The change's lines, in the activity feed.
  expect(last.lines.length, 'the change has lines').toBeGreaterThan(0)
  for (const line of last.lines) expect(screen.getByText(line)).toBeInTheDocument()
})

test('the contract file renders on the Downloads tab too', () => {
  const all: LiveMessage[] = messages
  const socket = new EngineSocket()
  render(<Page open={() => socket as unknown as WebSocket} tab="downloads" />)
  for (const message of all) socket.send(JSON.stringify(message))

  const { list } = lastDownloads(all)
  // The checks below prove nothing unless every kind of card is in the file.
  const states = list.map((d) => d.state)
  for (const state of ['starting', 'waiting', 'downloading', 'done', 'failed', 'stopped'] as const) {
    expect(states, `a ${state} download`).toContain(state)
  }
  expect(list.some((d) => d.note !== null), 'a download with a note').toBe(true)
  expect(list.some((d) => d.cables.some((c) => c.notes.length > 0 && c.bytes === 0)), 'a cable sitting out').toBe(true)

  // Every download, the newest first, by its name or else its address.
  const titles = list.map((d) => d.name ?? d.url)
  const cards = screen.getAllByRole('region').filter((r) => r.classList.contains('dl'))
  expect(cards.map((c) => c.getAttribute('aria-label'))).toEqual([...titles].reverse())

  for (const d of list) {
    const card = within(screen.getByRole('region', { name: d.name ?? d.url }))
    expect(card.getByTestId('state')).toHaveTextContent(new RegExp(`^${d.state}$`))
    expect(card.getByTestId('foot')).toHaveTextContent(`download ${d.id}`)
    if (d.note !== null) expect(card.getByTestId('note')).toHaveTextContent(d.note)
    if (d.state === 'downloading' && d.length !== null) {
      expect(card.getByTestId('progress')).toHaveTextContent(`${formatBytes(d.done)} of ${formatBytes(d.length)}`)
      expect(card.getByTestId('percent')).toHaveTextContent(`${percent(d.done, d.length)}%`)
    }
    // Each cable that carried bytes, holds connections or says something
    // new, with its bytes and its notes.
    const rows = card.queryAllByTestId('cable')
    for (const c of d.cables) {
      const row = rows.find((r) => within(r).getByTestId('cable-name').textContent === c.name)
      const notes = c.notes.filter((n) => n !== d.note)
      if (c.bytes === 0 && c.connections === 0 && notes.length === 0) {
        expect(row, `${c.name} on ${d.name ?? d.url} has nothing to say`).toBeUndefined()
        continue
      }
      expect(row, `${c.name} on ${d.name ?? d.url}`).toBeDefined()
      expect(within(row!).getByTestId('cable-bytes')).toHaveTextContent(formatBytes(c.bytes))
      for (const n of notes) expect(within(row!).getByText(n)).toBeInTheDocument()
    }
  }
})

// The page draws these from phase 4 on; the sample must keep sending one of
// each, or the page's handling of them is never tried against the engine.
test('the contract carries a metered cable, a cable wanting more than it holds, and the data used', () => {
  const tables = messages.flatMap((m) => (m.type === 'downloads' ? [] : [m.table]))
  expect(tables.some((t) => t.links.some((l) => l.metered))).toBe(true)
  const { list, used } = lastDownloads(messages)
  expect(list.some((d) => d.cables.some((c) => c.wanted > c.connections))).toBe(true)
  // Each cable that wrote has received at least what it wrote, once.
  expect(used.length).toBe(new Set(used.map((u) => u.cable)).size)
  for (const u of used) {
    const wrote = list.flatMap((d) => d.cables).filter((c) => c.name === u.cable)
    expect(u.bytes).toBeGreaterThanOrEqual(wrote.reduce((sum, c) => sum + c.bytes, 0))
  }
})

test("the contract's cards show the metered mark, the data used, and each cable's share and wanted", () => {
  const socket = new EngineSocket()
  const open = () => socket as unknown as WebSocket
  const { unmount } = render(<Page open={open} tab="links" />)
  for (const message of messages) socket.send(JSON.stringify(message))
  const last = messages[messages.length - 1]
  if (last.type !== 'change') throw new Error('the contract ends with a change')
  const { used, list } = lastDownloads(messages)

  for (const name of cableNames(last.table.links)) {
    const card = within(screen.getByRole('region', { name }))
    const metered = last.table.links.some((l) => l.name === name && l.metered)
    expect(card.queryByTestId('metered') !== null, `${name} metered`).toBe(metered)
    const bytes = used.find((u) => u.cable === name)?.bytes ?? 0
    if (bytes > 0) expect(card.getByTestId('used')).toHaveTextContent(`used ${formatBytes(bytes)}`)
    else expect(card.queryByTestId('used'), `${name} used`).toBeNull()
  }
  unmount()

  render(<Page open={open} tab="downloads" />)
  for (const message of messages) socket.send(JSON.stringify(message))
  for (const d of list.filter((d) => d.state === 'downloading')) {
    const card = within(screen.getByRole('region', { name: d.name ?? d.url }))
    for (const c of d.cables.filter((c) => c.connections > 0)) {
      const row = card.getAllByTestId('cable').find((r) => within(r).getByTestId('cable-name').textContent === c.name)!
      expect(within(row).getByTestId('cable-carrying')).toHaveTextContent(
        `carrying ${Math.floor((c.bps * 100) / d.bps)}%`,
      )
      if (c.wanted !== c.connections) expect(within(row).getByTestId('cable-wants')).toHaveTextContent(`wants ${c.wanted}`)
      else expect(within(row).queryByTestId('cable-wants')).toBeNull()
    }
  }
})
