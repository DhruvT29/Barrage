import { useRef, useState, type FormEvent } from 'react'

/** What the engine made of an address: the new download's id, or why not, in its words. */
type Added = { id: number } | { refused: string }

/**
 * Asks the engine that served the page to download `url`. The engine checks
 * the address and words every refusal (400, 409, 415); its words are passed
 * on as they are.
 */
async function addDownload(url: string): Promise<Added> {
  let answer: Response
  try {
    answer = await fetch('/api/downloads', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ url }),
    })
  } catch {
    return { refused: 'the engine is not answering' }
  }
  const body: unknown = await answer.json().catch(() => null)
  const { id, error } = (body ?? {}) as { id?: unknown; error?: unknown }
  if (answer.ok && typeof id === 'number') return { id }
  if (typeof error === 'string') return { refused: error }
  return { refused: `the engine answered ${answer.status}` }
}

/**
 * The field to add a download, and the engine's refusal under it. An address
 * the engine takes clears the field for the next one; a refused one stays,
 * to be put right.
 */
export function AddForm() {
  const [url, setUrl] = useState('')
  const [refused, setRefused] = useState<string | null>(null)
  const [sending, setSending] = useState(false)
  // A second press before the page redraws must not send the address twice:
  // the engine would answer that it is already downloading it.
  const busy = useRef(false)
  const wanted = url.trim()

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
    } else {
      setRefused(added.refused)
    }
  }

  return (
    <form className="panel add" aria-label="Add a download" noValidate onSubmit={submit}>
      <label className="label" htmlFor="add-url">
        Download a file from its link
      </label>
      <div className="field">
        <input
          id="add-url"
          type="url"
          inputMode="url"
          autoComplete="off"
          spellCheck={false}
          placeholder="https://example.com/file.iso"
          value={url}
          onChange={(event) => {
            setUrl(event.target.value)
            setRefused(null)
          }}
        />
        <button type="submit" disabled={wanted === '' || sending}>
          Download
        </button>
      </div>
      {refused !== null && (
        <p className="refused" role="alert">
          {refused}
        </p>
      )}
    </form>
  )
}
