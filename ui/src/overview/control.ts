import type { Priority } from '../api/types'

/** What the engine made of a stop or a resume: done, or why not, in its words. */
export type Asked = { ok: true } | { refused: string }

/** What a transfer can be asked by its address alone. */
type Action = 'stop' | 'resume' | 'remove'

/**
 * Asks the engine that served the page to stop download `id` (its progress
 * saved) or to resume it, as a card's Stop and Resume and the hero's Stop
 * all do. The body is `{}` as JSON, which a form on another site cannot
 * send; the engine's refusals (404, 409) are passed on as they are.
 */
export function controlDownload(id: number, action: Action): Promise<Asked> {
  return control(`/api/downloads/${id}/${action}`)
}

/** The same for torrent `id`. */
export function controlTorrent(id: number, action: Action): Promise<Asked> {
  return control(`/api/torrents/${id}/${action}`)
}

/** Asks the engine to give a download or a torrent this priority. */
export function setPriority(kind: 'web' | 'torrent', id: number, priority: Priority): Promise<Asked> {
  const route = kind === 'web' ? 'downloads' : 'torrents'
  return control(`/api/${route}/${id}/priority`, JSON.stringify({ priority }))
}

/** Asks the engine to run this many at once. */
export function setLimit(limit: number): Promise<Asked> {
  return control('/api/queue/limit', JSON.stringify({ limit }))
}

/** Asks the engine to zero the data used. */
export function resetUsed(): Promise<Asked> {
  return control('/api/used/reset')
}

async function control(address: string, body = '{}'): Promise<Asked> {
  let answer: Response
  try {
    answer = await fetch(address, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body,
    })
  } catch {
    return { refused: 'the engine is not answering' }
  }
  if (answer.ok) return { ok: true }
  const said: unknown = await answer.json().catch(() => null)
  const { error } = (said ?? {}) as { error?: unknown }
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
export function addDownload(url: string): Promise<Added> {
  return add('/api/downloads', 'application/json', JSON.stringify({ url }))
}

/** Asks the engine that served the page to fetch the torrent of a magnet link. */
export function addMagnet(magnet: string): Promise<Added> {
  return add('/api/torrents', 'application/json', JSON.stringify({ magnet }))
}

/** Asks the engine that served the page to fetch a chosen .torrent file, sent as its bytes. */
export function addTorrentFile(file: Blob): Promise<Added> {
  return add('/api/torrents', 'application/x-bittorrent', file)
}

async function add(address: string, contentType: string, sent: BodyInit): Promise<Added> {
  let answer: Response
  try {
    answer = await fetch(address, { method: 'POST', headers: { 'Content-Type': contentType }, body: sent })
  } catch {
    return { refused: 'the engine is not answering' }
  }
  const body: unknown = await answer.json().catch(() => null)
  const { id, error } = (body ?? {}) as { id?: unknown; error?: unknown }
  if (answer.ok && typeof id === 'number') return { id }
  if (typeof error === 'string') return { refused: error }
  return { refused: `the engine answered ${answer.status}` }
}
