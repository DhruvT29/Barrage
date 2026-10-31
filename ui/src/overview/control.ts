/** What the engine made of a stop or a resume: done, or why not, in its words. */
export type Asked = { ok: true } | { refused: string }

/**
 * Asks the engine that served the page to stop download `id` (its progress
 * saved) or to resume it, as a card's Stop and Resume and the hero's Stop
 * all do. The body is `{}` as JSON, which a form on another site cannot
 * send; the engine's refusals (404, 409) are passed on as they are.
 */
export async function controlDownload(id: number, action: 'stop' | 'resume'): Promise<Asked> {
  let answer: Response
  try {
    answer = await fetch(`/api/downloads/${id}/${action}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: '{}',
    })
  } catch {
    return { refused: 'the engine is not answering' }
  }
  if (answer.ok) return { ok: true }
  const body: unknown = await answer.json().catch(() => null)
  const { error } = (body ?? {}) as { error?: unknown }
  return { refused: typeof error === 'string' ? error : `the engine answered ${answer.status}` }
}

/** Stops download `id`, its progress saved. */
export function stopDownload(id: number): Promise<Asked> {
  return controlDownload(id, 'stop')
}

/** What the engine made of an address: the new download's id, or why not, in its words. */
export type Added = { id: number } | { refused: string }

/**
 * Asks the engine that served the page to download `url`. The engine checks
 * the address and words every refusal (400, 409, 415); its words are passed
 * on as they are.
 */
export async function addDownload(url: string): Promise<Added> {
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
