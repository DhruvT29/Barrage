import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, expect, test, vi } from 'vitest'

import type { CableShare, DownloadState, DownloadView } from '../api/types'
import { DownloadCard } from './DownloadCard'

function cable(name: string, fields: Partial<CableShare> = {}): CableShare {
  // Wanting what it holds, unless the test says otherwise.
  return { name, bytes: 0, bps: 0, connections: 0, notes: [], ...fields, wanted: fields.wanted ?? fields.connections ?? 0 }
}

function download(fields: Partial<DownloadView> = {}): DownloadView {
  return {
    id: 4,
    url: 'https://example.com/releases/ubuntu.iso',
    name: 'ubuntu.iso',
    state: 'downloading',
    length: 4_000_000_000,
    done: 1_200_000_000,
    bps: 189_900_000,
    note: null,
    cables: [],
    ...fields,
  }
}

const COLORS = { 'Wi-Fi': '#22ff66', 'Ethernet 2': '#22d3ee' }

const card = (name = 'ubuntu.iso') => screen.getByRole('region', { name })
const stat = (c: HTMLElement, id: string) => within(c).getByTestId(id).textContent
const toggle = (c: HTMLElement) => within(c).getByRole('button', { name: /^Link distribution/ })

/** Opens the card's list of cables, which a card not downloading starts with closed. */
function expand(c: HTMLElement) {
  if (toggle(c).getAttribute('aria-expanded') === 'false') fireEvent.click(toggle(c))
}

/** A cable's row, cell by cell, as it reads. */
function cells(row: HTMLElement) {
  const text = (id: string) => within(row).getByTestId(id).textContent
  return {
    name: text('cable-name'),
    bytes: text('cable-bytes'),
    speed: text('cable-speed'),
    // No detail line while a cable takes no part: its connections read empty.
    connections: within(row).queryByTestId('cable-connections')?.textContent ?? '',
    notes: within(row)
      .queryAllByTestId('cable-note')
      .map((n) => n.textContent),
  }
}

const SITS_OUT = 'sits out: the server gave this cable another version of the file ("7b1d", not "a3f0")'

test('a download card shows its state, id, name, host, progress, the five figures and each cable', () => {
  render(
    <DownloadCard
      colors={COLORS}
      download={download({
        cables: [
          cable('Wi-Fi', { bytes: 300_000_000, bps: 48_200_000, connections: 4 }),
          cable('Ethernet 2', { bytes: 900_000_000, bps: 141_700_000, connections: 8 }),
          cable('Ethernet 3', { notes: [SITS_OUT] }),
        ],
      })}
    />,
  )

  const c = card()
  expect(within(c).getByRole('heading', { name: 'ubuntu.iso' })).toBeInTheDocument()
  expect(stat(c, 'state')).toBe('downloading')
  expect(stat(c, 'id')).toBe('#4')
  expect(stat(c, 'host')).toBe('example.com')
  expect(within(c).getByTestId('host')).toHaveAttribute('title', 'https://example.com/releases/ubuntu.iso')

  expect(stat(c, 'percent')).toBe('30.0%')
  expect(within(c).getByRole('progressbar', { name: 'Progress' })).toHaveAttribute('aria-valuenow', '30')
  expect(stat(c, 'progress')).toBe('1.2 GB / 4.0 GB')
  expect(stat(c, 'speed')).toBe('189.9 Mbps')
  // 2.8 GB left at 189.9 Mbps: 117.97 s, rounded up.
  expect(stat(c, 'eta')).toBe('1m 58s')
  expect(stat(c, 'connections')).toBe('12')
  // Ethernet 3 sits out: it carries none of it.
  expect(stat(c, 'cables')).toBe('2')

  // Downloading, the list of cables starts open.
  expect(toggle(c)).toHaveAttribute('aria-expanded', 'true')
  expect(toggle(c)).toHaveTextContent('Link distribution · 3 cables')
  expect(within(c).getByRole('list', { name: 'Cables' })).toBeVisible()
  expect(within(c).getAllByTestId('cable').map(cells)).toEqual([
    { name: 'Wi-Fi', bytes: '300.0 MB', speed: '48.2 Mbps', connections: '4 connections', notes: [] },
    { name: 'Ethernet 2', bytes: '900.0 MB', speed: '141.7 Mbps', connections: '8 connections', notes: [] },
    { name: 'Ethernet 3', bytes: '0 B', speed: '-', connections: '', notes: [SITS_OUT] },
  ])
  // Each cable's share of the bytes so far, in its colour; one gone from the table is dim.
  expect(within(c).getAllByTestId('cable-share').map((s) => s.style.width)).toEqual(['25%', '75%', '0%'])
  expect(
    within(c)
      .getAllByTestId('cable')
      .map((row) => row.style.getPropertyValue('--c')),
  ).toEqual(['#22ff66', '#22d3ee', 'var(--dim)'])
})

test('the piece bar fills by cable colour, in proportion to what each carried', () => {
  render(
    <DownloadCard
      colors={COLORS}
      download={download({
        cables: [
          cable('Wi-Fi', { bytes: 300_000_000, bps: 48_200_000, connections: 4 }),
          cable('Ethernet 2', { bytes: 900_000_000, bps: 141_700_000, connections: 8 }),
        ],
      })}
    />,
  )
  // 64 blocks before the page has measured the bar: 30% fills 19, a quarter
  // of them Wi-Fi's (4.75, so 5), the rest Ethernet 2's.
  const blocks = within(card()).getAllByTestId('piece')
  expect(blocks).toHaveLength(64)
  const filled = blocks.filter((b) => b.classList.contains('on')).map((b) => b.style.getPropertyValue('--c'))
  expect(filled).toEqual([...Array(5).fill('#22ff66'), ...Array(14).fill('#22d3ee')])
})

test('without a length, the bar shimmers and the percent is "-"', () => {
  const whole =
    'the server sends this file only whole, not in pieces, so it comes through one cable, and cannot resume if that cable drops'
  render(
    <DownloadCard
      download={download({
        length: null,
        done: 12_300_000,
        note: whole,
        cables: [cable('Wi-Fi', { bytes: 12_300_000, bps: 30_000_000, connections: 1 })],
      })}
    />,
  )

  const c = card()
  expect(stat(c, 'percent')).toBe('-')
  expect(stat(c, 'progress')).toBe('12.3 MB / -')
  expect(stat(c, 'eta')).toBe('-')
  const bar = within(c).getByRole('progressbar', { name: 'Progress' })
  expect(bar).not.toHaveAttribute('aria-valuenow')
  expect(bar).toHaveClass('unknown')
  expect(within(c).queryAllByTestId('piece').filter((b) => b.classList.contains('on'))).toHaveLength(0)
  expect(within(c).getByTestId('note')).toHaveTextContent(whole)
})

test('"1 connection", not "1 connections"', () => {
  render(
    <DownloadCard
      download={download({ cables: [cable('Wi-Fi', { bytes: 1_200_000_000, bps: 48_200_000, connections: 1 })] })}
    />,
  )
  expect(within(card()).getByTestId('cable-connections')).toHaveTextContent(/^1 connection$/)
  expect(toggle(card())).toHaveTextContent('Link distribution · 1 cable')
})

test("a failed download shows the engine's note, once, in red", () => {
  const why =
    'finding https://example.com/files/gone.zip through 192.0.2.10: https://example.com/files/gone.zip answered 404 Not Found'
  render(
    <DownloadCard
      download={download({
        id: 2,
        url: 'https://example.com/files/gone.zip',
        name: null,
        state: 'failed',
        length: null,
        done: 0,
        bps: 0,
        note: why,
        cables: [cable('Wi-Fi', { notes: [why] })],
      })}
    />,
  )

  // Never found, so no name: the card goes by its address, written once.
  const c = card('https://example.com/files/gone.zip')
  expect(within(c).getAllByText('https://example.com/files/gone.zip')).toHaveLength(1)
  expect(within(c).queryByTestId('host')).toBeNull()
  expect(stat(c, 'id')).toBe('#2')
  expect(stat(c, 'state')).toBe('failed')
  expect(within(c).getByTestId('note')).toHaveTextContent(why)
  expect(within(c).getByTestId('note')).toHaveClass('failure')
  expect(within(c).getAllByText(why)).toHaveLength(1)
  // A cable that carried nothing and only repeats that note is not listed,
  // so there is nothing to open.
  expect(within(c).queryAllByTestId('cable')).toHaveLength(0)
  expect(within(c).queryByRole('button', { name: /^Link distribution/ })).toBeNull()
  expect(within(c).getByTestId('distribution')).toHaveTextContent('Link distribution · no cables')
  expect(stat(c, 'speed')).toBe('-')
  expect(stat(c, 'progress')).toBe('-')
  expect(stat(c, 'eta')).toBe('Failed')
  expect(stat(c, 'connections')).toBe('-')
  expect(stat(c, 'cables')).toBe('0')
})

test('a download that failed partway keeps what it got on screen', () => {
  render(
    <DownloadCard
      download={download({
        state: 'failed',
        bps: 0,
        note: 'the file changed on the server',
        cables: [
          cable('Wi-Fi', { bytes: 300_000_000, notes: ['the answer broke off: connection reset'] }),
          cable('Ethernet 2', { bytes: 900_000_000 }),
        ],
      })}
    />,
  )

  const c = card()
  expect(stat(c, 'progress')).toBe('1.2 GB / 4.0 GB')
  expect(stat(c, 'percent')).toBe('30.0%')
  expect(within(c).getByTestId('note')).toHaveTextContent(/^the file changed on the server$/)
  expect(toggle(c)).toHaveAttribute('aria-expanded', 'false')
  expand(c)
  expect(within(c).getAllByTestId('cable').map(cells)).toEqual([
    { name: 'Wi-Fi', bytes: '300.0 MB', speed: '', connections: '', notes: ['the answer broke off: connection reset'] },
    { name: 'Ethernet 2', bytes: '900.0 MB', speed: '', connections: '', notes: [] },
  ])
})

test('each state shows its word, not only a colour', () => {
  const states: DownloadState[] = ['starting', 'waiting', 'downloading', 'done', 'failed', 'stopped']
  for (const state of states) {
    const { unmount } = render(<DownloadCard download={download({ state })} />)
    const c = card()
    expect(within(c).getByTestId('state')).toHaveTextContent(new RegExp(`^${state}$`))
    // The colour comes from the state's class.
    expect(c).toHaveClass('tq-card', `tq-${state}`)
    unmount()
  }
})

test('starting and waiting say what they wait for, with no speed', () => {
  const starting = render(
    <DownloadCard
      download={download({
        name: null,
        url: 'https://example.com/a.pdf',
        state: 'starting',
        length: null,
        done: 0,
        bps: 0,
      })}
    />,
  )
  const s = card('https://example.com/a.pdf')
  expect(within(s).getByTestId('words')).toHaveTextContent(/^finding the file$/)
  expect(stat(s, 'speed')).toBe('-')
  expect(stat(s, 'progress')).toBe('-')
  expect(stat(s, 'eta')).toBe('Waiting')
  starting.unmount()

  // Waiting partway: what it has so far stays on screen.
  render(<DownloadCard download={download({ state: 'waiting', bps: 0 })} />)
  const w = card()
  expect(within(w).getByTestId('words')).toHaveTextContent(/^no cable to fetch it through yet$/)
  expect(stat(w, 'progress')).toBe('1.2 GB / 4.0 GB')
  expect(stat(w, 'speed')).toBe('-')
  expect(stat(w, 'eta')).toBe('Waiting')
})

test('a finished download shows its size, every block filled, and what each cable carried, without speeds', () => {
  render(
    <DownloadCard
      colors={COLORS}
      download={download({
        id: 1,
        url: 'https://example.com/dist/node-headers.tar.gz',
        name: 'node-headers.tar.gz',
        state: 'done',
        length: 8_762_927,
        done: 8_762_927,
        bps: 0,
        cables: [cable('Wi-Fi', { bytes: 2_190_000 }), cable('Ethernet 2', { bytes: 6_572_927 })],
      })}
    />,
  )

  const c = card('node-headers.tar.gz')
  expect(stat(c, 'progress')).toBe('8.8 MB / 8.8 MB')
  expect(stat(c, 'percent')).toBe('100.0%')
  expect(stat(c, 'speed')).toBe('-')
  expect(stat(c, 'eta')).toBe('Complete')
  expect(stat(c, 'cables')).toBe('2')
  expect(within(c).getAllByTestId('piece').every((b) => b.classList.contains('on'))).toBe(true)
  expand(c)
  expect(within(c).getAllByTestId('cable').map(cells)).toEqual([
    { name: 'Wi-Fi', bytes: '2.2 MB', speed: '', connections: '', notes: [] },
    { name: 'Ethernet 2', bytes: '6.6 MB', speed: '', connections: '', notes: [] },
  ])
})

// The list of cables: open while downloading, closed otherwise, and the user's choice.

test('the list of cables opens and closes, and starts closed once a download is not downloading', () => {
  const cables = [cable('Wi-Fi', { bytes: 300_000_000 })]
  const { rerender } = render(<DownloadCard download={download({ state: 'stopped', bps: 0, cables })} />)
  const c = card()
  // The toggle names the list it opens.
  const list = document.getElementById(toggle(c).getAttribute('aria-controls')!)!
  expect(list).toHaveAttribute('aria-label', 'Cables')
  expect(toggle(c)).toHaveAttribute('aria-expanded', 'false')
  expect(list).not.toBeVisible()
  expect(within(c).queryByRole('list', { name: 'Cables' })).toBeNull()

  fireEvent.click(toggle(c))
  expect(toggle(c)).toHaveAttribute('aria-expanded', 'true')
  expect(list).toBeVisible()

  // Resumed: the user's choice stands.
  fireEvent.click(toggle(c))
  rerender(<DownloadCard download={download({ cables })} />)
  expect(toggle(card())).toHaveAttribute('aria-expanded', 'false')
})

test('with no choice made, the list follows the state: it closes when the download finishes', () => {
  const cables = [cable('Wi-Fi', { bytes: 300_000_000, connections: 2 })]
  const { rerender } = render(<DownloadCard download={download({ cables })} />)
  expect(toggle(card())).toHaveAttribute('aria-expanded', 'true')
  rerender(<DownloadCard download={download({ state: 'done', done: 4_000_000_000, cables })} />)
  expect(toggle(card())).toHaveAttribute('aria-expanded', 'false')
})

test('a choice held by the queue wins, and is told to it', () => {
  const told: boolean[] = []
  const cables = [cable('Wi-Fi', { bytes: 300_000_000, connections: 2 })]
  render(<DownloadCard download={download({ cables })} expanded={false} onToggle={(open) => told.push(open)} />)
  expect(toggle(card())).toHaveAttribute('aria-expanded', 'false')
  fireEvent.click(toggle(card()))
  expect(told).toEqual([true])
})

// Stop and Resume (Task 13).

afterEach(() => {
  vi.unstubAllGlobals()
})

/** The engine answering each request with `status` and `body`; every request it got. */
function engine(status: number, body: unknown) {
  const asked: { url: string; init: RequestInit }[] = []
  vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
    asked.push({ url, init })
    return { ok: status >= 200 && status < 300, status, json: async () => body } as unknown as Response
  })
  return asked
}

/** The card's Stop or Resume button, whichever it has. */
const action = (c: HTMLElement) => within(c).queryByRole('button', { name: /^(Stop|Resume)$/ })

test('a running download can be stopped, a stopped one resumed, an ended one neither', () => {
  const buttons: [DownloadState, string | null][] = [
    ['starting', 'Stop'],
    ['waiting', 'Stop'],
    ['downloading', 'Stop'],
    ['stopped', 'Resume'],
    ['done', null],
    ['failed', null],
  ]
  for (const [state, word] of buttons) {
    const { unmount } = render(<DownloadCard download={download({ state })} />)
    const button = action(card())
    expect(button?.getAttribute('aria-label') ?? null, state).toBe(word)
    // An icon, said in words to a screen reader and on hover.
    if (button) expect(button).toHaveAttribute('title', word)
    unmount()
  }
})

test('Stop and Resume post {} as JSON to the download', async () => {
  const asked = engine(200, { id: 4 })
  const running = render(<DownloadCard download={download()} />)
  await act(async () => {
    fireEvent.click(action(card())!)
  })
  running.unmount()
  render(<DownloadCard download={download({ state: 'stopped' })} />)
  await act(async () => {
    fireEvent.click(action(card())!)
  })

  expect(asked.map((a) => a.url)).toEqual(['/api/downloads/4/stop', '/api/downloads/4/resume'])
  for (const { init } of asked) {
    expect(init.method).toBe('POST')
    expect(init.headers).toEqual({ 'Content-Type': 'application/json' })
    expect(init.body).toBe('{}')
  }
  // The card changes when the engine's next status says so, not before.
  expect(screen.queryByRole('alert')).toBeNull()
})

test("a refusal is shown in the engine's words", async () => {
  engine(409, { error: 'it is not running, so there is nothing to stop' })
  render(<DownloadCard download={download()} />)
  await act(async () => {
    fireEvent.click(action(card())!)
  })
  expect(within(card()).getByRole('alert')).toHaveTextContent(
    /^it is not running, so there is nothing to stop$/,
  )
})

test('a refusal goes once the download has moved on', async () => {
  engine(409, { error: 'it is not running, so there is nothing to stop' })
  const { rerender } = render(<DownloadCard download={download()} />)
  await act(async () => {
    fireEvent.click(action(card())!)
  })
  expect(within(card()).getByRole('alert')).toBeInTheDocument()
  // It finished before it heard: the refusal said so, and the card now does.
  rerender(<DownloadCard download={download({ state: 'done' })} />)
  expect(within(card()).queryByRole('alert')).toBeNull()
})

test('an engine that does not answer says so', async () => {
  vi.stubGlobal('fetch', async () => {
    throw new TypeError('Failed to fetch')
  })
  render(<DownloadCard download={download()} />)
  await act(async () => {
    fireEvent.click(action(card())!)
  })
  expect(within(card()).getByRole('alert')).toHaveTextContent(/^the engine is not answering$/)
})

test('a press waits for its answer: a second sends nothing', async () => {
  const asked: string[] = []
  vi.stubGlobal('fetch', (url: string) => {
    asked.push(url)
    return new Promise(() => {})
  })
  render(<DownloadCard download={download()} />)
  const button = action(card())!
  await act(async () => {
    fireEvent.click(button)
    fireEvent.click(button)
  })
  expect(asked).toEqual(['/api/downloads/4/stop'])
  expect(button).toBeDisabled()
})

test('a stopped download keeps what it got on screen, with the note, and no speeds', () => {
  render(
    <DownloadCard
      download={download({
        state: 'stopped',
        bps: 0,
        note: 'stopped; resuming carries on from here',
        cables: [cable('Wi-Fi', { bytes: 300_000_000 }), cable('Ethernet 2', { bytes: 900_000_000 })],
      })}
    />,
  )
  const c = card()
  expect(stat(c, 'progress')).toBe('1.2 GB / 4.0 GB')
  expect(stat(c, 'percent')).toBe('30.0%')
  expect(stat(c, 'speed')).toBe('-')
  expect(stat(c, 'eta')).toBe('Stopped')
  expect(within(c).getByTestId('note')).toHaveTextContent(/^stopped; resuming carries on from here$/)
  expect(within(c).getByTestId('note')).not.toHaveClass('failure')
  expand(c)
  expect(within(c).getAllByTestId('cable').map(cells)).toEqual([
    { name: 'Wi-Fi', bytes: '300.0 MB', speed: '', connections: '', notes: [] },
    { name: 'Ethernet 2', bytes: '900.0 MB', speed: '', connections: '', notes: [] },
  ])
})

/** The second line of each cable's row, by cable. */
function details(c: HTMLElement) {
  return within(c)
    .getAllByTestId('cable')
    .map((row) => [
      within(row).getByTestId('cable-name').textContent,
      within(row).queryByTestId('cable-carrying')?.textContent ?? null,
      within(row).queryByTestId('cable-wants')?.textContent ?? null,
    ])
}

test("while downloading, each cable's share of the speed, rounded down, and what it is wanted when that differs", () => {
  render(
    <DownloadCard
      download={download({
        bps: 126_900_000,
        cables: [
          cable('Wi-Fi', { bytes: 1_210_000_000, bps: 38_400_000, connections: 4, wanted: 6 }),
          // 69.74%: rounded down, 69.
          cable('Ethernet 2', { bytes: 1_317_000_000, bps: 88_500_000, connections: 8, wanted: 8 }),
          cable('Ethernet 3', { notes: [SITS_OUT] }),
        ],
      })}
    />,
  )
  expect(details(card())).toEqual([
    ['Wi-Fi', 'carrying 30%', 'wants 6'],
    ['Ethernet 2', 'carrying 69%', null],
    ['Ethernet 3', null, null],
  ])
  // The connections read as before, the wanted after them.
  const wifi = within(card()).getAllByTestId('cable')[0]
  expect(within(wifi).getByTestId('cable-connections')).toHaveTextContent(/^4 connections$/)
  expect(within(wifi).getByTestId('cable-detail')).toHaveTextContent(/^carrying 30% · 4 connections, wants 6$/)
})

test('fewer wanted than held shows too, and a cable holding none yet that is wanted some', () => {
  render(
    <DownloadCard
      download={download({
        cables: [
          cable('Wi-Fi', { bytes: 1, bps: 1_000_000, connections: 4, wanted: 2 }),
          cable('Ethernet 2', { bytes: 0, bps: 0, connections: 0, wanted: 3 }),
        ],
      })}
    />,
  )
  expect(details(card()).map((d) => d[2])).toEqual(['wants 2', 'wants 3'])
  const eth = within(card()).getAllByTestId('cable')[1]
  expect(within(eth).getByTestId('cable-detail')).toHaveTextContent(/^carrying 0% · 0 connections, wants 3$/)
})

test('no share of the speed and no wanted once a download is not downloading', () => {
  for (const state of ['done', 'stopped', 'failed', 'waiting', 'starting'] as const) {
    const { unmount } = render(
      <DownloadCard
        download={download({
          state,
          bps: 0,
          cables: [cable('Wi-Fi', { bytes: 300_000_000, bps: 0, connections: 0, wanted: 4 })],
        })}
      />,
    )
    expand(card())
    expect(details(card()), state).toEqual([['Wi-Fi', null, null]])
    // No empty detail line under the row either.
    expect(within(card()).queryByTestId('cable-detail'), state).toBeNull()
    unmount()
  }
})
