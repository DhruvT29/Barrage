import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, expect, test, vi } from 'vitest'

import type { TorrentCableView, TorrentsView, TorrentView } from '../api/types'
import { SwarmLine, TorrentCard } from './TorrentCard'

afterEach(() => {
  vi.unstubAllGlobals()
})

const HASH = '0123456789abcdef0123456789abcdef01234567'

function cable(name: string, fields: Partial<TorrentCableView> = {}): TorrentCableView {
  return { name, received: 1000, sent: 0, bps: 0, connections: 1, uploads: true, ...fields }
}

function torrent(fields: Partial<TorrentView> = {}): TorrentView {
  return {
    priority: 'normal',
    id: 7,
    name: 'distro.iso',
    info_hash: HASH,
    phase: 'downloading',
    length: 8000,
    done: 2000,
    down_bps: 8_000_000,
    up_bps: 1_000_000,
    peers: 12,
    note: null,
    cables: [cable('Wi-Fi')],
    ...fields,
  }
}

/** The engine's answer to the page's requests, and every request it got. */
function engine(status = 200, body: unknown = { id: 7 }) {
  const asked: { url: string; init: RequestInit }[] = []
  vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
    asked.push({ url, init })
    return { ok: status < 300, status, json: async () => body } as unknown as Response
  })
  return asked
}

test('the phase, the number, the name and the figures', () => {
  render(<TorrentCard torrent={torrent()} />)
  expect(screen.getByRole('heading', { name: 'distro.iso' })).toBeInTheDocument()
  expect(screen.getByTestId('state')).toHaveTextContent(/^downloading$/)
  expect(screen.getByTestId('id')).toHaveTextContent(/^#7$/)
  expect(screen.getByText('torrent')).toBeInTheDocument()
  expect(screen.getByTestId('percent')).toHaveTextContent('25.0%')
  expect(screen.getByTestId('down')).toHaveTextContent('8.0 Mbps')
  expect(screen.getByTestId('up')).toHaveTextContent('1.0 Mbps')
  expect(screen.getByTestId('peers')).toHaveTextContent(/^12$/)
})

test('the info hash shows until the name is known', () => {
  const { rerender } = render(
    <TorrentCard torrent={torrent({ name: null, phase: 'finding', length: null, done: 0, cables: [] })} />,
  )
  expect(screen.getByRole('heading', { name: HASH })).toBeInTheDocument()
  expect(screen.getByTestId('state')).toHaveTextContent(/^finding$/)
  expect(screen.getByTestId('words')).toHaveTextContent('fetching its details from peers')
  expect(screen.getByTestId('progress')).toHaveTextContent(/^-$/)
  expect(screen.getByTestId('eta')).toHaveTextContent(/^Finding$/)

  rerender(<TorrentCard torrent={torrent()} />)
  expect(screen.queryByText(HASH)).toBeNull()
  expect(screen.queryByTestId('words')).toBeNull()
})

test('a metered cable says upload off, and a cable that may upload does not', () => {
  render(
    <TorrentCard
      torrent={torrent({
        cables: [
          cable('Wi-Fi', { received: 3000, sent: 2000, connections: 10 }),
          cable('Ethernet 2', { received: 1000, connections: 1, uploads: false }),
        ],
      })}
    />,
  )
  const [wifi, phone] = screen.getAllByTestId('cable')
  expect(within(wifi).getByTestId('cable-detail')).toHaveTextContent('sent 2.0 kB · 10 connections')
  expect(within(wifi).queryByTestId('upload-off')).toBeNull()
  expect(within(phone).getByTestId('cable-detail')).toHaveTextContent('1 connection')
  expect(within(phone).getByTestId('upload-off')).toHaveTextContent('upload off: metered')
})

test('the bar and the rows split by what each cable received', () => {
  render(
    <TorrentCard
      torrent={torrent({
        done: 4000,
        cables: [cable('Wi-Fi', { received: 3000 }), cable('Ethernet 2', { received: 1000 })],
      })}
      colors={{ 'Wi-Fi': 'red', 'Ethernet 2': 'blue' }}
    />,
  )
  const shares = screen.getAllByTestId('cable-share').map((s) => s.style.width)
  expect(shares).toEqual(['75%', '25%'])
  const on = screen.getAllByTestId('piece').filter((p) => p.classList.contains('on'))
  const colours = on.map((p) => p.style.getPropertyValue('--c'))
  expect(colours.filter((c) => c === 'red').length).toBe(3 * colours.filter((c) => c === 'blue').length)
})

test('a failed torrent shows the engine\'s words, and a done one its whole size', () => {
  const { rerender } = render(<TorrentCard torrent={torrent({ phase: 'failed', note: 'the disk is full' })} />)
  expect(screen.getByTestId('note')).toHaveTextContent('the disk is full')
  expect(screen.queryByRole('button', { name: 'Stop' })).toBeNull()
  expect(screen.queryByRole('button', { name: 'Resume' })).toBeNull()

  rerender(<TorrentCard torrent={torrent({ phase: 'done', done: 8000 })} />)
  expect(screen.getByTestId('percent')).toHaveTextContent('100.0%')
  expect(screen.getByTestId('eta')).toHaveTextContent(/^Complete$/)
  expect(screen.getByTestId('down')).toHaveTextContent(/^-$/)
})

test('stop and resume call the torrent routes', async () => {
  const asked = engine()
  const { rerender } = render(<TorrentCard torrent={torrent()} />)
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
  })
  expect(asked.map((a) => a.url)).toEqual(['/api/torrents/7/stop'])
  expect(asked[0].init).toMatchObject({ method: 'POST', body: '{}' })

  rerender(<TorrentCard torrent={torrent({ phase: 'stopped' })} />)
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Resume' }))
  })
  expect(asked.map((a) => a.url)).toEqual(['/api/torrents/7/stop', '/api/torrents/7/resume'])
})

test('a refused stop shows the engine\'s words until the phase moves on', async () => {
  engine(409, { error: 'it is not running, so there is nothing to stop' })
  const { rerender } = render(<TorrentCard torrent={torrent()} />)
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
  })
  expect(screen.getByRole('alert')).toHaveTextContent('it is not running, so there is nothing to stop')
  rerender(<TorrentCard torrent={torrent({ phase: 'done' })} />)
  expect(screen.queryByRole('alert')).toBeNull()
})

function swarm(listening: TorrentsView['listening'], udp_cable: string | null = 'Wi-Fi'): TorrentsView {
  return { list: [], udp_cable, listening }
}

test('the swarm line names the UDP cable and the port, in words', () => {
  const words = (view: TorrentsView) => {
    const { unmount } = render(<SwarmLine torrents={view} />)
    const text = screen.getByTestId('swarm').textContent
    unmount()
    return text
  }
  expect(words(swarm({ state: 'trying', cable: 'Wi-Fi', why: null }))).toBe(
    'DHT and UDP on Wi-Fi · incoming: trying on Wi-Fi',
  )
  expect(words(swarm({ state: 'open', cable: 'Wi-Fi', why: null }))).toBe(
    'DHT and UDP on Wi-Fi · incoming: open on Wi-Fi',
  )
  expect(words(swarm({ state: 'off', cable: null, why: null }, null))).toBe(
    'DHT and UDP wait for a working cable · incoming: off',
  )
  expect(words(swarm({ state: 'closed', cable: 'Wi-Fi', why: 'nobody called in' }))).toBe(
    'DHT and UDP on Wi-Fi · incoming: closednobody called in',
  )
})
