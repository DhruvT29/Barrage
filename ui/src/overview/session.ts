import { useEffect, useState } from 'react'

// When this page got its first table from the engine. It lives as long as
// the page does, not as long as a component: the hero is drawn again each
// time its tab is shown, and the time it shows must not start over. It is
// the page's own connection time, never the engine's uptime, which the
// engine does not send.
let firstTable: number | null = null

/** Forgets the first table, as a reload does. For tests. */
export function forgetFirstTable() {
  firstTable = null
}

/**
 * Milliseconds since the page got its first table, ticking once a second;
 * null before it has one.
 */
export function useConnectedFor(hasTable: boolean): number | null {
  const [since, setSince] = useState<number | null>(firstTable)
  const [now, setNow] = useState(() => Date.now())

  useEffect(() => {
    if (!hasTable) return
    if (firstTable === null) firstTable = Date.now()
    setSince(firstTable)
    setNow(Date.now())
  }, [hasTable])

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [])

  return since === null ? null : Math.max(0, now - since)
}
