import { useEffect, useId, useRef, useState, type FormEvent, type KeyboardEvent } from 'react'

import { CloseIcon, LinkIcon } from '../shell/icons'
import { addDownload } from './control'
import './modal.css'

/** What can take focus inside the window, in order. */
const FOCUSABLE = 'button:not([disabled]), input:not([disabled]), [href], [tabindex]:not([tabindex="-1"])'

/**
 * The add-a-download window, opened from the hero and the queue. Closed, it
 * is nothing at all; each opening starts from an empty field.
 */
export function AddModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  return open ? <Window onClose={onClose} /> : null
}

/**
 * The window itself: the field, the engine's refusal under it, and
 * Download. An address the engine takes closes the window; a refused one
 * stays, to be put right. Escape, the close button or a click outside close
 * it; focus starts in the field, stays inside while it is open, and goes
 * back to whatever opened it.
 */
function Window({ onClose }: { onClose: () => void }) {
  const [url, setUrl] = useState('')
  const [refused, setRefused] = useState<string | null>(null)
  const [sending, setSending] = useState(false)
  // A second press before the page redraws must not send the address twice:
  // the engine would answer that it is already downloading it.
  const busy = useRef(false)
  const panel = useRef<HTMLDivElement>(null)
  const field = useRef<HTMLInputElement>(null)
  const pressedOutside = useRef(false)
  const close = useRef(onClose)
  close.current = onClose
  const id = useId()
  const wanted = url.trim()

  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null
    field.current?.focus()
    const { overflow } = document.body.style
    document.body.style.overflow = 'hidden'
    const onEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape') close.current()
    }
    document.addEventListener('keydown', onEscape)
    return () => {
      document.removeEventListener('keydown', onEscape)
      document.body.style.overflow = overflow
      opener?.focus()
    }
  }, [])

  // Tab and Shift+Tab wrap around inside the window.
  function trap(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key !== 'Tab' || !panel.current) return
    const inside = [...panel.current.querySelectorAll<HTMLElement>(FOCUSABLE)]
    if (inside.length === 0) return
    const first = inside[0]
    const last = inside[inside.length - 1]
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault()
      last.focus()
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault()
      first.focus()
    }
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (wanted === '' || busy.current) return
    busy.current = true
    setSending(true)
    const added = await addDownload(wanted)
    busy.current = false
    setSending(false)
    if ('id' in added) {
      setUrl('')
      setRefused(null)
      onClose()
    } else {
      setRefused(added.refused)
    }
  }

  return (
    <div
      className="modal-backdrop"
      data-testid="modal-backdrop"
      // Only a press that starts and ends outside the window closes it, so
      // selecting the address and letting go outside does not.
      onMouseDown={(event) => {
        pressedOutside.current = event.target === event.currentTarget
      }}
      onClick={(event) => {
        if (pressedOutside.current && event.target === event.currentTarget) onClose()
        pressedOutside.current = false
      }}
    >
      <div
        ref={panel}
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={`${id}-title`}
        aria-describedby={`${id}-says`}
        onKeyDown={trap}
      >
        <span className="modal-corner tl" aria-hidden="true" />
        <span className="modal-corner br" aria-hidden="true" />

        <div className="modal-head">
          <h2 id={`${id}-title`} className="modal-title">
            <LinkIcon />
            Add a download
          </h2>
          <button type="button" className="modal-close" aria-label="Close" onClick={onClose}>
            <CloseIcon />
          </button>
        </div>

        <form noValidate onSubmit={submit}>
          <div className="modal-body">
            <p id={`${id}-says`} className="modal-says">
              The file is split across every working connection the moment it lands in the queue.
            </p>

            <label className="modal-label" htmlFor={`${id}-url`}>
              Link
            </label>
            <input
              ref={field}
              id={`${id}-url`}
              className="modal-field"
              type="url"
              inputMode="url"
              autoComplete="off"
              spellCheck={false}
              placeholder="https://example.com/file.iso"
              aria-describedby={refused === null ? `${id}-hint` : `${id}-hint ${id}-refused`}
              aria-invalid={refused !== null}
              value={url}
              onChange={(event) => {
                setUrl(event.target.value)
                setRefused(null)
              }}
            />
            <p id={`${id}-hint`} className="modal-hint">
              An http or https address. A stopped download of the same address resumes where it left off.
            </p>
            {refused !== null && (
              <p id={`${id}-refused`} className="modal-refused" role="alert">
                {refused}
              </p>
            )}
          </div>

          <div className="modal-foot">
            <button type="submit" className="modal-go" disabled={wanted === '' || sending} aria-busy={sending}>
              Download
            </button>
          </div>
        </form>
      </div>
    </div>
  )
}
