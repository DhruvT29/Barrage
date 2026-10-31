import { useEffect, useRef, useState } from 'react'

import type { DownloadsView, TableView } from './types'

/** Samples kept per series: a minute at one a second. */
export const HISTORY_LEN = 60

/** How often a sample is taken, in milliseconds. */
export const SAMPLE_MS = 1000

/**
 * The last minute of speeds, oldest first, in bits per second. `live` is
 * what the downloads fetch through each cable (0 while it carries nothing);
 * `measured` is each cable's measured speed (0 until measured). A cable that
 * appears later has zeros before it appeared, so every series is as long as
 * `liveTotal`.
 */
export type SpeedHistory = {
  liveTotal: number[]
  measuredTotal: number[]
  live: Record<string, number[]>
  measured: Record<string, number[]>
}

export const EMPTY_HISTORY: SpeedHistory = { liveTotal: [], measuredTotal: [], live: {}, measured: {} }

/** The measured speed of each cable by name, as `cables.ts` picks it: its first usable address with a figure. */
function measuredByCable(table: TableView | null): Record<string, number> {
  const out: Record<string, number> = {}
  for (const link of table?.links ?? []) {
    if (!(link.name in out)) out[link.name] = 0
    if (out[link.name] === 0 && (link.state === 'up' || link.state === 'degraded') && link.down_bps !== null) {
      out[link.name] = link.down_bps
    }
  }
  return out
}

function push(series: number[] | undefined, value: number, length: number, max: number): number[] {
  const padded = series ?? new Array<number>(length).fill(0)
  return [...padded, value].slice(-max)
}

/**
 * One more sample on `prev`. Pure, so it is tested without timers. A cable
 * gone from both the table and the downloads keeps its series (with zeros)
 * until it has been gone for a whole history.
 */
export function sample(
  prev: SpeedHistory,
  table: TableView | null,
  downloads: DownloadsView | null,
  max: number = HISTORY_LEN,
): SpeedHistory {
  const length = prev.liveTotal.length
  const measuredNow = measuredByCable(table)
  const liveNow: Record<string, number> = {}
  for (const part of downloads?.live.parts ?? []) liveNow[part.name] = part.bps

  const names = new Set([...Object.keys(prev.live), ...Object.keys(measuredNow), ...Object.keys(liveNow)])
  const live: Record<string, number[]> = {}
  const measured: Record<string, number[]> = {}
  for (const name of names) {
    live[name] = push(prev.live[name], liveNow[name] ?? 0, length, max)
    measured[name] = push(prev.measured[name], measuredNow[name] ?? 0, length, max)
    const gone = !(name in measuredNow) && !(name in liveNow)
    if (gone && live[name].every((v) => v === 0) && measured[name].every((v) => v === 0)) {
      delete live[name]
      delete measured[name]
    }
  }

  return {
    liveTotal: push(prev.liveTotal, downloads?.live.bps ?? 0, length, max),
    measuredTotal: push(prev.measuredTotal, table?.combined.bps ?? 0, length, max),
    live,
    measured,
  }
}

/**
 * The speed history, one sample a second from the latest table and
 * downloads. The page draws its sparklines and the hero's chart from it.
 */
export function useSpeedHistory(table: TableView | null, downloads: DownloadsView | null): SpeedHistory {
  const [history, setHistory] = useState<SpeedHistory>(EMPTY_HISTORY)
  const latest = useRef({ table, downloads })
  latest.current = { table, downloads }

  useEffect(() => {
    const tick = () => setHistory((prev) => sample(prev, latest.current.table, latest.current.downloads))
    const timer = setInterval(tick, SAMPLE_MS)
    return () => clearInterval(timer)
  }, [])

  return history
}
