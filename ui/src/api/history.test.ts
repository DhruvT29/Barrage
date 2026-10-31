import { describe, expect, it } from 'vitest'

import { EMPTY_HISTORY, sample } from './history'
import type { DownloadsView, LinkView, TableView } from './types'

function link(id: number, name: string, down_bps: number | null, state: LinkView['state'] = 'up'): LinkView {
  return {
    id,
    name,
    address: `192.0.2.${id}`,
    family: 'ipv4',
    state,
    rtt_ms: 20,
    down_bps,
    public_ip: null,
    metered: false,
  }
}

function table(links: LinkView[], bps: number): TableView {
  return { links, shared: [], connections: [], combined: { bps, parts: [], not_counted: [] } }
}

function downloads(parts: { name: string; bps: number }[]): DownloadsView {
  return { live: { bps: parts.reduce((a, p) => a + p.bps, 0), parts }, list: [], used: [] }
}

describe('sample', () => {
  it('records each cable and the totals', () => {
    const h = sample(EMPTY_HISTORY, table([link(1, 'Wi-Fi', 50e6), link(2, 'Ethernet', null)], 50e6), downloads([{ name: 'Wi-Fi', bps: 9e6 }]))
    expect(h.liveTotal).toEqual([9e6])
    expect(h.measuredTotal).toEqual([50e6])
    expect(h.live).toEqual({ 'Wi-Fi': [9e6], Ethernet: [0] })
    expect(h.measured).toEqual({ 'Wi-Fi': [50e6], Ethernet: [0] })
  })

  it('pads a cable that appears later so every series is as long', () => {
    let h = sample(EMPTY_HISTORY, table([link(1, 'Wi-Fi', 50e6)], 50e6), null)
    h = sample(h, table([link(1, 'Wi-Fi', 50e6), link(2, 'Ethernet', 80e6)], 130e6), null)
    expect(h.measured.Ethernet).toEqual([0, 80e6])
    expect(h.measured['Wi-Fi']).toEqual([50e6, 50e6])
  })

  it('keeps only the last max samples', () => {
    let h = EMPTY_HISTORY
    for (let i = 1; i <= 5; i++) h = sample(h, table([], 0), downloads([{ name: 'Wi-Fi', bps: i }]), 3)
    expect(h.liveTotal).toEqual([3, 4, 5])
    expect(h.live['Wi-Fi']).toEqual([3, 4, 5])
  })

  it('takes the measured speed only from a usable address', () => {
    const h = sample(EMPTY_HISTORY, table([link(1, 'Wi-Fi', 70e6, 'down'), link(2, 'Wi-Fi', 40e6)], 40e6), null)
    expect(h.measured['Wi-Fi']).toEqual([40e6])
  })

  it('drops a cable once it has been gone for a whole history', () => {
    let h = sample(EMPTY_HISTORY, table([link(1, 'Wi-Fi', 50e6)], 50e6), null, 2)
    h = sample(h, table([], 0), null, 2)
    expect(h.measured['Wi-Fi']).toEqual([50e6, 0])
    h = sample(h, table([], 0), null, 2)
    expect(h.measured).toEqual({})
  })
})
