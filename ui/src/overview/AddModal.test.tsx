import { act, fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'
import { afterEach, expect, test, vi } from 'vitest'

import { AddModal } from './AddModal'

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

/** A page with a button that opens the window, as the hero and the queue do. */
function Page({ onClose = () => {} }: { onClose?: () => void }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <button onClick={() => setOpen(true)}>Open</button>
      <AddModal
        open={open}
        onClose={() => {
          onClose()
          setOpen(false)
        }}
      />
    </>
  )
}

function openWindow(onClose?: () => void) {
  render(<Page onClose={onClose} />)
  const opener = screen.getByRole('button', { name: 'Open' })
  opener.focus()
  fireEvent.click(opener)
  return opener
}

const dialog = () => screen.getByRole('dialog', { name: 'Add a download' })
const field = () => screen.getByRole('textbox', { name: 'Link' })
const button = () => screen.getByRole('button', { name: 'Download' })

/** Types an address, presses Download, and lets the answer arrive. */
async function add(url: string) {
  fireEvent.change(field(), { target: { value: url } })
  await act(async () => {
    fireEvent.click(button())
  })
}

test('closed, there is no window', () => {
  render(<AddModal open={false} onClose={() => {}} />)
  expect(screen.queryByRole('dialog')).toBeNull()
})

test('open: a modal window, the field focused, with a hint', () => {
  openWindow()
  expect(dialog()).toHaveAttribute('aria-modal', 'true')
  expect(field()).toHaveFocus()
  expect(field()).toHaveAccessibleDescription(/http or https/)
})

test("adding posts the URL as JSON and shows the engine's refusal in its own words", async () => {
  const asked = engine(
    { status: 409, body: { error: 'already downloading, as download 4', id: 4 } },
    { status: 400, body: { error: 'only http and https addresses can be downloaded, not ftp:' } },
  )
  openWindow()

  await add('https://example.com/releases/distro.iso')
  expect(asked).toEqual([
    {
      url: '/api/downloads',
      init: {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ url: 'https://example.com/releases/distro.iso' }),
      },
    },
  ])
  expect(screen.getByRole('alert')).toHaveTextContent(/^already downloading, as download 4$/)
  // Refused, the address stays, to be put right, and the window stays open.
  expect(field()).toHaveValue('https://example.com/releases/distro.iso')
  expect(dialog()).toBeInTheDocument()

  await add('ftp://example.com/file.iso')
  expect(screen.getByRole('alert')).toHaveTextContent(/^only http and https addresses can be downloaded, not ftp:$/)
})

test('an address the engine takes clears the field and closes the window, focus back on the opener', async () => {
  const asked = engine({ status: 201, body: { id: 5 } })
  const onClose = vi.fn()
  const opener = openWindow(onClose)

  await add('  https://example.com/a.iso  ')
  expect(JSON.parse(String(asked[0].init.body))).toEqual({ url: 'https://example.com/a.iso' })
  expect(onClose).toHaveBeenCalledOnce()
  expect(screen.queryByRole('dialog')).toBeNull()
  expect(opener).toHaveFocus()

  // Opened again, the field is empty for the next one.
  fireEvent.click(opener)
  expect(field()).toHaveValue('')
  expect(screen.queryByRole('alert')).toBeNull()
})

test('changing the address clears the last refusal', async () => {
  engine({ status: 409, body: { error: 'already downloading, as download 4', id: 4 } })
  openWindow()

  await add('https://example.com/a.iso')
  expect(screen.getByRole('alert')).toBeInTheDocument()
  fireEvent.change(field(), { target: { value: 'https://example.com/b.iso' } })
  expect(screen.queryByRole('alert')).toBeNull()
})

test('an engine that answers oddly, or not at all, is said in words', async () => {
  engine({ status: 502, text: 'Bad Gateway' })
  openWindow()

  await add('https://example.com/a.iso')
  expect(screen.getByRole('alert')).toHaveTextContent(/^the engine answered 502$/)

  // No answers left: the request itself fails.
  await add('https://example.com/b.iso')
  expect(screen.getByRole('alert')).toHaveTextContent(/^the engine is not answering$/)
  expect(field()).toHaveValue('https://example.com/b.iso')
})

test('an empty address cannot be sent', () => {
  const asked = engine()
  openWindow()

  expect(button()).toBeDisabled()
  fireEvent.change(field(), { target: { value: '   ' } })
  expect(button()).toBeDisabled()
  // Enter in the field submits the form without the button.
  fireEvent.submit(field().closest('form')!)
  expect(asked).toHaveLength(0)
})

test('Download waits while an address is on its way, and a second press sends nothing', async () => {
  const asked: string[] = []
  vi.stubGlobal('fetch', (url: string) => {
    asked.push(url)
    return new Promise(() => {})
  })
  openWindow()

  await add('https://example.com/a.iso')
  expect(button()).toBeDisabled()
  await act(async () => {
    fireEvent.submit(field().closest('form')!)
  })
  expect(asked).toHaveLength(1)
})

test('Escape, the close button and a click on the backdrop each close it; a click inside does not', () => {
  const onClose = vi.fn()
  const opener = openWindow(onClose)

  fireEvent.mouseDown(dialog())
  fireEvent.click(dialog())
  expect(onClose).not.toHaveBeenCalled()

  fireEvent.keyDown(field(), { key: 'Escape' })
  expect(onClose).toHaveBeenCalledTimes(1)
  expect(opener).toHaveFocus()

  fireEvent.click(opener)
  fireEvent.click(screen.getByRole('button', { name: 'Close' }))
  expect(onClose).toHaveBeenCalledTimes(2)

  fireEvent.click(opener)
  const backdrop = screen.getByTestId('modal-backdrop')
  fireEvent.mouseDown(backdrop)
  fireEvent.click(backdrop)
  expect(onClose).toHaveBeenCalledTimes(3)
})

test('Tab stays inside the window', () => {
  openWindow()
  fireEvent.change(field(), { target: { value: 'https://example.com/a.iso' } })
  const close = screen.getByRole('button', { name: 'Close' })

  // From the last control, Tab wraps to the first; Shift+Tab from the first wraps to the last.
  button().focus()
  fireEvent.keyDown(button(), { key: 'Tab' })
  expect(close).toHaveFocus()
  fireEvent.keyDown(close, { key: 'Tab', shiftKey: true })
  expect(button()).toHaveFocus()
})
