import { act, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, expect, test, vi } from 'vitest'

import type { DownloadView, DownloadsView } from '../api/types'
import { Downloads } from './Downloads'

afterEach(() => {
  vi.unstubAllGlobals()
})

type Answer = { status: number; body: unknown } | { status: number; text: string }

/** The engine's answers to the page's requests, in order, and every request it got. */
function engine(...answers: Answer[]) {
  const asked: { url: string; init: RequestInit }[] = []
  vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
    asked.push({ url, init })
    const answer = answers.shift()
    // A browser's fetch fails like this when nothing is listening.
    if (!answer) throw new TypeError('Failed to fetch')
    const text = 'text' in answer ? answer.text : JSON.stringify(answer.body)
    return {
      ok: answer.status >= 200 && answer.status < 300,
      status: answer.status,
      json: async () => JSON.parse(text),
    } as unknown as Response
  })
  return asked
}

const NOTHING: DownloadsView = { live: { bps: 0, parts: [] }, list: [] }

function download(id: number, name: string): DownloadView {
  return {
    id,
    url: `https://example.com/${name}`,
    name,
    state: 'done',
    length: 1000,
    done: 1000,
    bps: 0,
    note: null,
    cables: [],
  }
}

const field = () => screen.getByRole('textbox', { name: 'Download a file from its link' })
const button = () => screen.getByRole('button', { name: 'Download' })

/** Types an address, presses Download, and lets the answer arrive. */
async function add(url: string) {
  fireEvent.change(field(), { target: { value: url } })
  await act(async () => {
    fireEvent.click(button())
  })
}

test("adding posts the URL as JSON and shows the engine's refusal in its own words", async () => {
  const asked = engine(
    { status: 409, body: { error: 'already downloading, as download 4', id: 4 } },
    { status: 400, body: { error: 'only http and https addresses can be downloaded, not ftp:' } },
  )
  render(<Downloads downloads={NOTHING} />)

  await add('https://example.com/releases/ubuntu.iso')
  expect(asked).toEqual([
    {
      url: '/api/downloads',
      init: {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ url: 'https://example.com/releases/ubuntu.iso' }),
      },
    },
  ])
  expect(screen.getByRole('alert')).toHaveTextContent(/^already downloading, as download 4$/)
  // Refused, the address stays, to be put right.
  expect(field()).toHaveValue('https://example.com/releases/ubuntu.iso')

  await add('ftp://example.com/file.iso')
  expect(screen.getByRole('alert')).toHaveTextContent(/^only http and https addresses can be downloaded, not ftp:$/)
})

test('an address the engine takes leaves the field empty for the next', async () => {
  const asked = engine({ status: 201, body: { id: 5 } })
  render(<Downloads downloads={NOTHING} />)

  await add('  https://example.com/a.iso  ')
  expect(JSON.parse(String(asked[0].init.body))).toEqual({ url: 'https://example.com/a.iso' })
  expect(field()).toHaveValue('')
  expect(screen.queryByRole('alert')).toBeNull()
})

test('changing the address clears the last refusal', async () => {
  engine({ status: 409, body: { error: 'already downloading, as download 4', id: 4 } })
  render(<Downloads downloads={NOTHING} />)

  await add('https://example.com/a.iso')
  expect(screen.getByRole('alert')).toBeInTheDocument()
  fireEvent.change(field(), { target: { value: 'https://example.com/b.iso' } })
  expect(screen.queryByRole('alert')).toBeNull()
})

test('an engine that answers oddly, or not at all, is said in words', async () => {
  engine({ status: 502, text: 'Bad Gateway' })
  render(<Downloads downloads={NOTHING} />)

  await add('https://example.com/a.iso')
  expect(screen.getByRole('alert')).toHaveTextContent(/^the engine answered 502$/)

  // No answers left: the request itself fails.
  await add('https://example.com/b.iso')
  expect(screen.getByRole('alert')).toHaveTextContent(/^the engine is not answering$/)
  expect(field()).toHaveValue('https://example.com/b.iso')
})

test('an empty address cannot be sent', () => {
  const asked = engine()
  render(<Downloads downloads={NOTHING} />)

  expect(button()).toBeDisabled()
  fireEvent.change(field(), { target: { value: '   ' } })
  expect(button()).toBeDisabled()
  // Enter in the field submits the form without the button.
  fireEvent.submit(field().closest('form')!)
  expect(asked).toHaveLength(0)
})

test('Download waits while an address is on its way', async () => {
  vi.stubGlobal('fetch', () => new Promise(() => {}))
  render(<Downloads downloads={NOTHING} />)

  await add('https://example.com/a.iso')
  expect(button()).toBeDisabled()
})

test('the newest download comes first', () => {
  const list = [download(1, 'a.iso'), download(2, 'b.iso'), download(3, 'c.iso')]
  render(<Downloads downloads={{ ...NOTHING, list }} />)
  expect(screen.getAllByRole('region').map((r) => r.getAttribute('aria-label'))).toEqual(['c.iso', 'b.iso', 'a.iso'])
})

test('no downloads yet: says so, and how', () => {
  render(<Downloads downloads={NOTHING} />)
  expect(screen.getByText(/Nothing downloaded yet/)).toHaveTextContent(
    'Nothing downloaded yet. Paste a link to a file above.',
  )
})

test('before the engine has said anything: the field, and no empty-list message', () => {
  render(<Downloads downloads={null} />)
  expect(field()).toBeInTheDocument()
  expect(screen.queryByText(/Nothing downloaded yet/)).toBeNull()
  expect(screen.queryAllByRole('region')).toHaveLength(0)
})
