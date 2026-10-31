import { act, render, screen, within } from '@testing-library/react'
import { expect, test } from 'vitest'

import { formatBytes, percentText } from '../downloads/format'
import { Dashboard } from '../links/Dashboard'
import { Hero } from '../overview/Hero'
import { colorsByName } from '../palette'
import type { Tab } from '../tabs'
import { messages } from './contract'
import { EMPTY_HISTORY } from './history'
import type { DownloadsView, LinkView, LiveMessage, TableView } from './types'
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

/** The page on `tab`, fed every message in the contract; `each` runs after each one, with the page. */
function feedAll(tab: Tab, each?: (message: LiveMessage, page: HTMLElement) => void) {
  const socket = new EngineSocket()
  const { container } = render(<Page open={() => socket as unknown as WebSocket} tab={tab} />)
  for (const message of messages as LiveMessage[]) {
    socket.send(JSON.stringify(message))
    each?.(message, container)
  }
  return container
}

/** The cables, by name, in the order they first appear. */
const cableNames = (links: LinkView[]) => [...new Set(links.map((l) => l.name))]

/** The last downloads the engine sent. */
function lastDownloads(all: LiveMessage[]): DownloadsView {
  const sent = all.flatMap((m) => (m.type === 'downloads' ? [m.downloads] : []))
  expect(sent.length, 'a downloads message').toBeGreaterThan(0)
  return sent[sent.length - 1]
}

/** The last change the engine sent: the contract ends with one. */
function lastChange(): Extract<LiveMessage, { type: 'change' }> {
  const last = messages[messages.length - 1] as LiveMessage
  if (last.type !== 'change') throw new Error('the contract ends with a change')
  return last
}

/** One cable's card in the download manager's left column. */
const cableCard = (name: string) => within(screen.getByRole('region', { name: 'Mesh links' })).getByRole('region', { name })

test('the download manager draws every message the engine really sends: the cables and the hero', () => {
  // A downloads message carries no table: the cards stay as the last table drew them.
  let drawn: LinkView[] = []
  feedAll('downloads', (message, page) => {
    if (message.type !== 'downloads') drawn = message.table.links
    expect(page.querySelectorAll('section.ml-card')).toHaveLength(cableNames(drawn).length)
  })

  const { table } = lastChange()
  // The loops below prove nothing over an empty file.
  expect(table.links.length, 'links').toBeGreaterThan(0)

  // Every cable, with each of its addresses in order.
  for (const name of cableNames(table.links)) {
    const shown = within(cableCard(name)).getAllByTestId('address')
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
  const card = within(cableCard(mixed!))
  expect(card.getByTestId('state')).toHaveTextContent(/^degraded$/)
  expect(card.getByText('portal')).toBeInTheDocument()

  // The cables on one connection are named in the notice.
  expect(table.connections.length, 'a shared connection').toBeGreaterThan(0)
  const notice = screen.getByTestId('shared-notice')
  for (const name of table.connections.flat()) expect(notice).toHaveTextContent(name)

  // A download is running, so the hero is its live speed, by cable.
  const { live } = lastDownloads(messages)
  expect(live.parts.length, 'cables carrying data').toBeGreaterThan(0)
  const running = within(screen.getByRole('region', { name: 'Downloading at' }))
  expect(running.getByTestId('total')).toHaveTextContent(`${(live.bps / 1e6).toFixed(1)} Mbps`)
  expect(running.getAllByTestId('part').map((p) => p.querySelector('b')?.textContent)).toEqual(
    live.parts.map((p) => p.name),
  )
})

test('with nothing downloading, the hero is the combined speed: its parts, and what it left out and why', () => {
  const { table } = lastChange()
  const { bps, parts, not_counted } = table.combined
  expect(parts.length, 'combined parts').toBeGreaterThan(0)
  expect(not_counted.length, 'cables left out of the sum').toBeGreaterThan(0)

  const colors = colorsByName(cableNames(table.links))
  render(
    <Hero
      status="live"
      table={table}
      feed={[]}
      downloads={null}
      history={EMPTY_HISTORY}
      colors={colors}
      onAdd={() => {}}
      onTab={() => {}}
    />,
  )
  const hero = within(screen.getByRole('region', { name: 'Combined speed available' }))
  expect(hero.getByTestId('total')).toHaveTextContent(`${(bps / 1e6).toFixed(1)} Mbps`)
  expect(hero.getAllByTestId('part').map((p) => p.querySelector('b')?.textContent)).toEqual(parts.map((p) => p.name))
  const note = hero.getByTestId('note')
  for (const other of parts.flatMap((p) => p.shared_with)) expect(note).toHaveTextContent(other)
  for (const { name, why } of not_counted) expect(note).toHaveTextContent(`${name} (${why})`)
})

test('the radar draws every message the engine really sends: a node and a roster row per cable, and the change lines', () => {
  let drawn: TableView | null = null
  const container = feedAll('links', (message, page) => {
    if (message.type !== 'downloads') drawn = message.table
    const names = drawn ? cableNames(drawn.links) : []
    expect(page.querySelectorAll('.rd-node')).toHaveLength(names.length)
    expect(page.querySelectorAll('.rd-row')).toHaveLength(names.length)
  })

  const last = lastChange()
  // Each node is named with its cable and its state in words.
  for (const name of cableNames(last.table.links)) {
    const node = [...container.querySelectorAll('.rd-node')].find((n) => n.getAttribute('aria-label')!.startsWith(`${name}:`))
    expect(node, `${name}'s node`).toBeDefined()
  }

  // The change's lines, in the activity feed.
  expect(last.lines.length, 'the change has lines').toBeGreaterThan(0)
  const activity = within(screen.getByRole('complementary', { name: 'Activity' }))
  for (const line of last.lines) expect(activity.getByText(line)).toBeInTheDocument()
})

test('the contract file renders on the download manager: every download card', () => {
  feedAll('downloads')

  const { list } = lastDownloads(messages)
  // The checks below prove nothing unless every kind of card is in the file.
  const states = list.map((d) => d.state)
  for (const state of ['starting', 'waiting', 'downloading', 'done', 'failed', 'stopped'] as const) {
    expect(states, `a ${state} download`).toContain(state)
  }
  expect(list.some((d) => d.note !== null), 'a download with a note').toBe(true)
  expect(list.some((d) => d.cables.some((c) => c.notes.length > 0 && c.bytes === 0)), 'a cable sitting out').toBe(true)

  // Every download, the newest first, by its name or else its address.
  const titles = list.map((d) => d.name ?? d.url)
  const queue = screen.getByRole('region', { name: 'Transfer queue' })
  const cards = within(queue)
    .getAllByRole('region')
    .filter((r) => r.classList.contains('tq-card'))
  expect(cards.map((c) => c.getAttribute('aria-label'))).toEqual([...titles].reverse())

  for (const d of list) {
    const card = within(within(queue).getByRole('region', { name: d.name ?? d.url }))
    expect(card.getByTestId('state')).toHaveTextContent(new RegExp(`^${d.state}$`))
    expect(card.getByTestId('id')).toHaveTextContent(new RegExp(`^#${d.id}$`))
    if (d.note !== null) expect(card.getByTestId('note')).toHaveTextContent(d.note)
    if (d.state === 'downloading' && d.length !== null) {
      expect(card.getByTestId('progress')).toHaveTextContent(`${formatBytes(d.done)} / ${formatBytes(d.length)}`)
      expect(card.getByTestId('percent')).toHaveTextContent(percentText(d.done, d.length))
    }
    // Each cable that carried bytes, holds connections, is wanted by a
    // running download or says something new, with its bytes and its notes.
    const rows = card.queryAllByTestId('cable')
    for (const c of d.cables) {
      const row = rows.find((r) => within(r).getByTestId('cable-name').textContent === c.name)
      const notes = c.notes.filter((n) => n !== d.note)
      const wanted = d.state === 'downloading' && c.wanted > 0
      if (c.bytes === 0 && c.connections === 0 && notes.length === 0 && !wanted) {
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
  feedAll('downloads')
  const last = lastChange()
  const { used, list } = lastDownloads(messages)

  for (const name of cableNames(last.table.links)) {
    const card = within(cableCard(name))
    const metered = last.table.links.some((l) => l.name === name && l.metered)
    expect(card.queryByTestId('metered') !== null, `${name} metered`).toBe(metered)
    if (metered) expect(card.getByTestId('metered')).toHaveTextContent('metered')
    const bytes = used.find((u) => u.cable === name)?.bytes ?? 0
    expect(card.getByTestId('stat-used'), `${name} used`).toHaveTextContent(bytes > 0 ? formatBytes(bytes) : /^Used-$/)
  }

  const queue = screen.getByRole('region', { name: 'Transfer queue' })
  for (const d of list.filter((d) => d.state === 'downloading')) {
    const card = within(within(queue).getByRole('region', { name: d.name ?? d.url }))
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
