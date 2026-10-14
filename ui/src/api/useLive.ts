import { useEffect, useState } from 'react'

import type { LiveMessage, TableView } from './types'

/** `connecting` until the first table, `live` while tables arrive, `disconnected` once lost. */
export type Status = 'connecting' | 'live' | 'disconnected'

/** One line of the activity feed: the engine's own words, and when they arrived. */
export type FeedLine = { at: Date; text: string }

/** How long to wait before each retry; the last one repeats for as long as it takes. */
export const RETRY_MS = [1000, 2000, 4000, 5000]

/** The feed keeps this many lines, newest first. */
export const FEED_MAX = 200

/** The engine that served this page. */
function ownEngine(): string {
  return `ws://${window.location.host}/api/live`
}

function connect(url: string): WebSocket {
  return new WebSocket(url)
}

/**
 * The engine's live table, kept connected: when the socket is lost, the last
 * table stays on screen and the hook tries again, waiting longer each time,
 * until the engine answers. One socket at a time, always.
 *
 * `open` exists for tests; it must be the same function on every render.
 */
export function useLive(
  url: string = ownEngine(),
  open: (url: string) => WebSocket = connect,
): { status: Status; table: TableView | null; feed: FeedLine[] } {
  const [status, setStatus] = useState<Status>('connecting')
  const [table, setTable] = useState<TableView | null>(null)
  const [feed, setFeed] = useState<FeedLine[]>([])

  useEffect(() => {
    let socket: WebSocket | null = null
    let retry: ReturnType<typeof setTimeout> | undefined
    // Attempts since the last table: an engine that accepts the socket but
    // never sends one is no better than one that refuses it.
    let failures = 0

    const start = () => {
      const s = open(url)
      socket = s
      s.onmessage = (event) => {
        const message = JSON.parse(event.data) as LiveMessage
        failures = 0
        // Each socket starts with a whole table: it replaces whatever the
        // last socket left, nothing is merged.
        setTable(message.table)
        setStatus('live')
        if (message.type === 'change') {
          const at = new Date()
          const lines = message.lines.map((text) => ({ at, text }))
          setFeed((feed) => [...lines, ...feed].slice(0, FEED_MAX))
        }
      }
      // Only a close retries. Browsers report one loss as an error and then a
      // close, so retrying on both would open two sockets.
      s.onclose = () => {
        socket = null
        setStatus('disconnected')
        retry = setTimeout(start, RETRY_MS[Math.min(failures, RETRY_MS.length - 1)])
        failures += 1
      }
    }

    start()
    return () => {
      clearTimeout(retry)
      if (socket) {
        // Closing on purpose is not a loss: no retry, no state change.
        socket.onclose = null
        socket.onmessage = null
        socket.close()
      }
    }
  }, [url, open])

  return { status, table, feed }
}
