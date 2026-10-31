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

const card = (name = 'ubuntu.iso') => screen.getByRole('region', { name })

/** A cable's row, cell by cell, as it reads. */
function cells(row: HTMLElement) {
  const text = (id: string) => within(row).getByTestId(id).textContent
  return {
    name: text('cable-name'),
    bytes: text('cable-bytes'),
    speed: text('cable-speed'),
    connections: text('cable-connections'),
    notes: within(row)
      .queryAllByTestId('cable-note')
      .map((n) => n.textContent),
  }
}

const SITS_OUT = 'sits out: the server gave this cable another version of the file ("7b1d", not "a3f0")'

test('a download card shows progress, bytes of total, speed and each cable', () => {
  render(
    <DownloadCard
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
  expect(within(c).getByTestId('state')).toHaveTextContent(/^downloading$/)
  expect(within(c).getByTestId('speed')).toHaveTextContent(/^189\.9 Mbps$/)
  expect(within(c).getByTestId('progress')).toHaveTextContent(/^1\.2 GB of 4\.0 GB$/)
  expect(within(c).getByTestId('percent')).toHaveTextContent(/^30%$/)
  expect(within(c).getByTestId('progress-fill')).toHaveStyle({ width: '30%' })

  expect(within(c).getAllByTestId('cable').map(cells)).toEqual([
    { name: 'Wi-Fi', bytes: '300.0 MB', speed: '48.2 Mbps', connections: '4 connections', notes: [] },
    { name: 'Ethernet 2', bytes: '900.0 MB', speed: '141.7 Mbps', connections: '8 connections', notes: [] },
    { name: 'Ethernet 3', bytes: '0 B', speed: '-', connections: '', notes: [SITS_OUT] },
  ])
  // Each cable's share of the bytes so far.
  expect(within(c).getAllByTestId('cable-share').map((s) => s.style.width)).toEqual(['25%', '75%', '0%'])

  const foot = within(c).getByTestId('foot')
  expect(foot).toHaveTextContent('https://example.com/releases/ubuntu.iso')
  expect(foot).toHaveTextContent('download 4')
})

test('"1 connection", not "1 connections"', () => {
  render(
    <DownloadCard
      download={download({ cables: [cable('Wi-Fi', { bytes: 1_200_000_000, bps: 48_200_000, connections: 1 })] })}
    />,
  )
  expect(within(card()).getByTestId('cable-connections')).toHaveTextContent(/^1 connection$/)
})

test("a failed download shows the engine's note, once", () => {
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
  expect(within(c).getByTestId('state')).toHaveTextContent(/^failed$/)
  expect(within(c).getByTestId('note')).toHaveTextContent(why)
  expect(within(c).getAllByText(why)).toHaveLength(1)
  // A cable that carried nothing and only repeats that note is not listed.
  expect(within(c).queryAllByTestId('cable')).toHaveLength(0)
  expect(within(c).queryByTestId('speed')).toBeNull()
  expect(within(c).queryByTestId('progress')).toBeNull()
  expect(within(c).getByTestId('foot')).toHaveTextContent(/^download 2$/)
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
  expect(within(c).getByTestId('progress')).toHaveTextContent(/^1\.2 GB of 4\.0 GB$/)
  expect(within(c).getByTestId('percent')).toHaveTextContent(/^30%$/)
  expect(within(c).getByTestId('note')).toHaveTextContent(/^the file changed on the server$/)
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
    // The colour and the shape come from the state's class.
    expect(c).toHaveClass('dl', state)
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
  expect(within(s).queryByTestId('speed')).toBeNull()
  expect(within(s).queryByTestId('progress')).toBeNull()
  starting.unmount()

  // Waiting partway: what it has so far stays on screen.
  render(<DownloadCard download={download({ state: 'waiting', bps: 0 })} />)
  const w = card()
  expect(within(w).getByTestId('words')).toHaveTextContent(/^no cable to fetch it through yet$/)
  expect(within(w).getByTestId('progress')).toHaveTextContent(/^1\.2 GB of 4\.0 GB$/)
  expect(within(w).queryByTestId('speed')).toBeNull()
})

test('a finished download shows its size and what each cable carried, without speeds', () => {
  render(
    <DownloadCard
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
  expect(within(c).getByTestId('progress')).toHaveTextContent(/^8\.8 MB$/)
  expect(within(c).queryByTestId('percent')).toBeNull()
  expect(within(c).queryByTestId('speed')).toBeNull()
  expect(within(c).getByTestId('progress-fill')).toHaveStyle({ width: '100%' })
  expect(within(c).getAllByTestId('cable').map(cells)).toEqual([
    { name: 'Wi-Fi', bytes: '2.2 MB', speed: '', connections: '', notes: [] },
    { name: 'Ethernet 2', bytes: '6.6 MB', speed: '', connections: '', notes: [] },
  ])
})

test('with no length, a download shows the bytes so far, and its note', () => {
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
  expect(within(c).getByTestId('progress')).toHaveTextContent(/^12\.3 MB$/)
  expect(within(c).queryByTestId('percent')).toBeNull()
  expect(within(c).queryByTestId('progress-fill')).toBeNull()
  expect(within(c).getByTestId('note')).toHaveTextContent(whole)
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

const action = (c: HTMLElement) => within(c).queryByRole('button')

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
    expect(action(card())?.textContent ?? null, state).toBe(word)
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
  expect(within(c).getByTestId('progress')).toHaveTextContent(/^1\.2 GB of 4\.0 GB$/)
  expect(within(c).getByTestId('percent')).toHaveTextContent(/^30%$/)
  expect(within(c).queryByTestId('speed')).toBeNull()
  expect(within(c).getByTestId('note')).toHaveTextContent(/^stopped; resuming carries on from here$/)
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
          cable('Ethernet 2', { bytes: 1, bps: 0, connections: 0, wanted: 3 }),
        ],
      })}
    />,
  )
  expect(details(card()).map((d) => d[2])).toEqual(['wants 2', 'wants 3'])
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
    expect(details(card()), state).toEqual([['Wi-Fi', null, null]])
    unmount()
  }
})
