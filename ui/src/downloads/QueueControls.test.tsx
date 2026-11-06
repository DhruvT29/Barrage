import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'

import { messages } from '../api/contract'
import type { DownloadsView, DownloadView, LiveMessage, QueueView, TorrentsView, TorrentView } from '../api/types'
import { stoppable, stoppableTorrents } from '../overview/figures'
import { EMPTY_HISTORY } from '../api/history'
import { DownloadCard } from './DownloadCard'
import { etaText, forgetChoices, inLine, placeOf, torrentEta } from './queue'
import { TorrentCard } from './TorrentCard'
import { TransferQueue } from './TransferQueue'

function download(fields: Partial<DownloadView> = {}): DownloadView {
  return {
    id: 4,
    url: 'https://example.com/ubuntu.iso',
    name: 'ubuntu.iso',
    state: 'queued',
    length: null,
    done: 0,
    bps: 0,
    note: null,
    cables: [],
    priority: 'normal',
    ...fields,
  }
}

function torrent(fields: Partial<TorrentView> = {}): TorrentView {
  return {
    id: 7,
    name: 'debian.iso',
    info_hash: 'a'.repeat(40),
    phase: 'queued',
    length: null,
    done: 0,
    down_bps: 0,
    up_bps: 0,
    peers: 0,
    note: null,
    cables: [],
    priority: 'normal',
    ...fields,
  }
}

const QUEUE: QueueView = { limit: 3, most: 20, holding: 2, places: [], notice: null, used_since: 1_700_000_000 }

function page(list: DownloadView[], swarm: TorrentView[], queue: QueueView = QUEUE) {
  const downloads: DownloadsView = { live: { bps: 0, parts: [] }, list, used: [] }
  const torrents: TorrentsView = { list: swarm, udp_cable: null, listening: { state: 'off', cable: null, why: null } }
  return { status: 'live' as const, table: null, feed: [], downloads, torrents, queue, history: EMPTY_HISTORY, colors: {} }
}

let posted: { url: string; body: unknown }[]
let answer: () => Response

beforeEach(() => {
  posted = []
  answer = () => new Response('{}', { status: 200 })
  forgetChoices()
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init: RequestInit) => {
      posted.push({ url, body: JSON.parse(init.body as string) })
      return answer()
    }),
  )
})

afterEach(() => {
  vi.unstubAllGlobals()
})

test('a place in line is written out', () => {
  expect(inLine(1)).toBe('1st in line')
  expect(inLine(2)).toBe('2nd in line')
  expect(inLine(3)).toBe('3rd in line')
  expect(inLine(4)).toBe('4th in line')
  expect(inLine(11)).toBe('11th in line')
  expect(inLine(12)).toBe('12th in line')
  expect(inLine(13)).toBe('13th in line')
  expect(inLine(22)).toBe('22nd in line')
})

test('the time left of a queued transfer is its place, or Queued without one', () => {
  expect(etaText(download(), 3)).toBe('3rd in line')
  expect(etaText(download())).toBe('Queued')
  expect(torrentEta(torrent(), 1)).toBe('1st in line')
  expect(torrentEta(torrent())).toBe('Queued')
})

test('a transfer finds its place by kind and id', () => {
  const queue = { ...QUEUE, places: [{ kind: 'web', id: 4, place: 1 }, { kind: 'torrent', id: 4, place: 2 }] } satisfies QueueView
  expect(placeOf(queue, 'web', 4)).toBe(1)
  expect(placeOf(queue, 'torrent', 4)).toBe(2)
  expect(placeOf(queue, 'web', 9)).toBeUndefined()
  expect(placeOf(null, 'web', 4)).toBeUndefined()
})

test('Stop all takes queued transfers too', () => {
  const { downloads, torrents } = page([download()], [torrent()])
  expect(stoppable(downloads)).toHaveLength(1)
  expect(stoppableTorrents(torrents)).toHaveLength(1)
})

test('a queued card offers Stop', () => {
  render(<DownloadCard download={download()} place={2} />)
  expect(screen.getByRole('button', { name: 'Stop' })).toBeInTheDocument()
  expect(screen.getByText('2nd in line')).toBeInTheDocument()

  fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
  expect(posted[0].url).toBe('/api/downloads/4/stop')
})

test('a queued torrent offers Stop', () => {
  render(<TorrentCard torrent={torrent()} place={1} />)
  expect(screen.getByRole('button', { name: 'Stop' })).toBeInTheDocument()
  expect(screen.getByText('1st in line')).toBeInTheDocument()
})

test('the priority is changed on the card', async () => {
  render(<DownloadCard download={download()} />)
  const picker = screen.getByRole('combobox', { name: 'Priority' })
  expect(picker).toHaveValue('normal')

  fireEvent.change(picker, { target: { value: 'high' } })

  await waitFor(() => expect(posted).toHaveLength(1))
  expect(posted[0]).toEqual({ url: '/api/downloads/4/priority', body: { priority: 'high' } })
})

test('a torrent changes its own priority', async () => {
  render(<TorrentCard torrent={torrent()} />)

  fireEvent.change(screen.getByRole('combobox', { name: 'Priority' }), { target: { value: 'low' } })

  await waitFor(() => expect(posted).toHaveLength(1))
  expect(posted[0]).toEqual({ url: '/api/torrents/7/priority', body: { priority: 'low' } })
})

test('a refused priority is shown in the engine words', async () => {
  answer = () => new Response(JSON.stringify({ error: 'it has ended' }), { status: 409 })
  render(<DownloadCard download={download()} />)

  fireEvent.change(screen.getByRole('combobox', { name: 'Priority' }), { target: { value: 'high' } })

  expect(await screen.findByRole('alert')).toHaveTextContent('it has ended')
})

test('a transfer that has ended has no priority to change', () => {
  render(<DownloadCard download={download({ state: 'done' })} />)
  expect(screen.queryByRole('combobox', { name: 'Priority' })).not.toBeInTheDocument()
})

test('Remove is on every card', () => {
  for (const state of ['queued', 'starting', 'waiting', 'downloading', 'done', 'failed', 'stopped'] as const) {
    const { unmount } = render(<DownloadCard download={download({ state })} />)
    expect(screen.getByRole('button', { name: 'Remove' })).toBeInTheDocument()
    unmount()
  }
  render(<TorrentCard torrent={torrent({ phase: 'done' })} />)
  expect(screen.getByRole('button', { name: 'Remove' })).toBeInTheDocument()
})

test('a transfer that is not running is removed at once', async () => {
  render(<DownloadCard download={download({ state: 'done' })} />)

  fireEvent.click(screen.getByRole('button', { name: 'Remove' }))

  await waitFor(() => expect(posted).toHaveLength(1))
  expect(posted[0]).toEqual({ url: '/api/downloads/4/remove', body: {} })
})

test('a queued one is removed at once too: nothing is lost', async () => {
  render(<TorrentCard torrent={torrent()} />)

  fireEvent.click(screen.getByRole('button', { name: 'Remove' }))

  await waitFor(() => expect(posted).toHaveLength(1))
  expect(posted[0]).toEqual({ url: '/api/torrents/7/remove', body: {} })
})

test('a running transfer asks first, on the card', async () => {
  render(<DownloadCard download={download({ state: 'downloading' })} />)

  fireEvent.click(screen.getByRole('button', { name: 'Remove' }))
  expect(posted).toHaveLength(0)
  expect(screen.getByText('Stop it and remove it? Its files stay on disk.')).toBeInTheDocument()

  fireEvent.click(screen.getByRole('button', { name: 'Keep it' }))
  expect(screen.queryByText('Stop it and remove it? Its files stay on disk.')).not.toBeInTheDocument()
  expect(posted).toHaveLength(0)

  fireEvent.click(screen.getByRole('button', { name: 'Remove' }))
  fireEvent.click(screen.getByRole('button', { name: 'Stop and remove' }))
  await waitFor(() => expect(posted).toHaveLength(1))
  expect(posted[0].url).toBe('/api/downloads/4/remove')
})

test('a refused removal is shown in the engine words', async () => {
  answer = () => new Response(JSON.stringify({ error: 'no such download' }), { status: 404 })
  render(<DownloadCard download={download({ state: 'done' })} />)

  fireEvent.click(screen.getByRole('button', { name: 'Remove' }))

  expect(await screen.findByRole('alert')).toHaveTextContent('no such download')
})

test('the queue says how many run, and its limit can be changed', async () => {
  render(<TransferQueue {...page([download()], [])} onAdd={() => {}} />)
  expect(screen.getByTestId('running')).toHaveTextContent('2 of 3 running')
  const picker = screen.getByRole('combobox', { name: 'Run at once' })
  expect(picker).toHaveValue('3')
  expect(within(picker).getAllByRole('option')).toHaveLength(20)

  fireEvent.change(picker, { target: { value: '5' } })

  await waitFor(() => expect(posted).toHaveLength(1))
  expect(posted[0]).toEqual({ url: '/api/queue/limit', body: { limit: 5 } })
})

test('each card shows its place from the queue', () => {
  const queue: QueueView = {
    ...QUEUE,
    places: [
      { kind: 'torrent', id: 7, place: 1 },
      { kind: 'web', id: 4, place: 2 },
    ],
  }
  render(<TransferQueue {...page([download()], [torrent()], queue)} onAdd={() => {}} />)
  expect(screen.getByText('1st in line')).toBeInTheDocument()
  expect(screen.getByText('2nd in line')).toBeInTheDocument()
})

test('a notice is shown until it is dismissed, and a new one shows again', () => {
  const saved = { ...QUEUE, notice: 'The saved list could not be read.' }
  const { rerender } = render(<TransferQueue {...page([], [], saved)} onAdd={() => {}} />)
  expect(screen.getByRole('status')).toHaveTextContent('The saved list could not be read.')

  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }))
  expect(screen.queryByRole('status')).not.toBeInTheDocument()

  rerender(<TransferQueue {...page([], [], { ...QUEUE, notice: 'Something else.' })} onAdd={() => {}} />)
  expect(screen.getByRole('status')).toHaveTextContent('Something else.')
})

test('the data used says since when, and resets once asked', async () => {
  render(<TransferQueue {...page([download()], [])} onAdd={() => {}} />)
  const line = screen.getByTestId('used-since')
  expect(line).toHaveTextContent(`Data used since ${new Date(1_700_000_000_000).toLocaleDateString()}`)

  fireEvent.click(within(line).getByRole('button', { name: 'Reset' }))
  expect(posted).toHaveLength(0)
  fireEvent.click(within(line).getByRole('button', { name: 'Reset to zero' }))

  await waitFor(() => expect(posted).toHaveLength(1))
  expect(posted[0]).toEqual({ url: '/api/used/reset', body: {} })
})

test('the engine really sends a queue the page can draw', () => {
  const sent = messages as LiveMessage[]
  const queue = sent.flatMap((m) => (m.type === 'queue' ? [m.queue] : []))
  const list = sent.flatMap((m) => (m.type === 'downloads' ? m.downloads.list : []))
  const swarm = sent.flatMap((m) => (m.type === 'torrents' ? m.torrents.list : []))
  expect(queue).toHaveLength(1)

  render(<TransferQueue {...page(list, swarm, queue[0])} onAdd={() => {}} />)

  expect(screen.getByText('1st in line')).toBeInTheDocument()
  expect(screen.getByText('2nd in line')).toBeInTheDocument()
  expect(screen.getByRole('status')).toHaveTextContent('could not be read')
})
