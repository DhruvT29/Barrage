import { expect, test } from 'vitest'

import type { CableShare, DownloadState, DownloadView } from '../api/types'
import { blockCount, countByFilter, etaText, filterOf, pieceBlocks } from './queue'

function cable(name: string, bytes: number): CableShare {
  return { name, bytes, bps: 0, connections: 0, wanted: 0, notes: [] }
}

function download(fields: Partial<DownloadView> = {}): DownloadView {
  return {
    id: 1,
    url: 'https://example.com/a.iso',
    name: 'a.iso',
    state: 'downloading',
    length: 1000,
    done: 300,
    bps: 8_000,
    note: null,
    cables: [],
    ...fields,
  }
}

const COLORS = { 'Wi-Fi': '#wifi', Ethernet: '#eth' }

test('the filled blocks are split between the cables by their bytes, in the order of the cables', () => {
  // 30% of 10 blocks: 3. Wi-Fi carried a quarter (0.75 of a block), Ethernet
  // three quarters (2.25): the spare block goes to the larger remainder.
  const blocks = pieceBlocks(
    download({ cables: [cable('Wi-Fi', 75), cable('Ethernet', 225)] }),
    COLORS,
    10,
  )
  expect(blocks).toEqual(['#wifi', '#eth', '#eth', null, null, null, null, null, null, null])
})

test('the order is the order of download.cables, not the order of the colours', () => {
  const blocks = pieceBlocks(
    download({ done: 500, cables: [cable('Ethernet', 200), cable('Wi-Fi', 300)] }),
    COLORS,
    10,
  )
  expect(blocks).toEqual(['#eth', '#eth', '#wifi', '#wifi', '#wifi', null, null, null, null, null])
})

test('a cable that carried nothing has no blocks; one gone from the table is dim', () => {
  const blocks = pieceBlocks(
    download({ done: 400, cables: [cable('Wi-Fi', 0), cable('Ethernet 9', 400)] }),
    COLORS,
    5,
  )
  expect(blocks).toEqual(['var(--dim)', 'var(--dim)', null, null, null])
})

test('blocks fill rounded down: only a finished download fills every block', () => {
  expect(pieceBlocks(download({ done: 999, cables: [cable('Wi-Fi', 999)] }), COLORS, 10)?.filter(Boolean)).toHaveLength(9)
  expect(pieceBlocks(download({ done: 0 }), COLORS, 10)?.filter(Boolean)).toHaveLength(0)
  const done = pieceBlocks(download({ state: 'done', done: 1000, cables: [cable('Wi-Fi', 1000)] }), COLORS, 10)
  expect(done).toEqual(Array(10).fill('#wifi'))
})

test('filled blocks no cable accounts for are green', () => {
  expect(pieceBlocks(download({ state: 'done', length: 0, done: 0 }), COLORS, 4)).toEqual(Array(4).fill('var(--green)'))
})

test('without a length there is nothing to fill against: no blocks, unless it is done', () => {
  expect(pieceBlocks(download({ length: null, done: 500 }), COLORS, 10)).toBeNull()
  expect(pieceBlocks(download({ state: 'done', length: null, done: 500, cables: [cable('Ethernet', 500)] }), COLORS, 3)).toEqual([
    '#eth',
    '#eth',
    '#eth',
  ])
})

test('the number of blocks fits the width: 10 px blocks 3 px apart, 16 to 80 of them', () => {
  expect(blockCount(1000)).toBe(77)
  expect(blockCount(2000)).toBe(80)
  expect(blockCount(100)).toBe(16)
  expect(blockCount(0)).toBe(16)
})

test('the time left comes from the speed and the bytes left, rounded up', () => {
  // 700 bytes left at 8000 bits per second: 0.7 s, so 1s.
  expect(etaText(download())).toBe('1s')
  expect(etaText(download({ length: 34_200_000_000, done: 16_600_000_000, bps: 197_800_000 }))).toBe('11m 52s')
})

test('the time left is "-" while downloading without a speed or a length, and a word in every other state', () => {
  expect(etaText(download({ bps: 0 }))).toBe('-')
  expect(etaText(download({ length: null }))).toBe('-')
  const words: [DownloadState, string][] = [
    ['starting', 'Waiting'],
    ['waiting', 'Waiting'],
    ['stopped', 'Stopped'],
    ['done', 'Complete'],
    ['failed', 'Failed'],
  ]
  for (const [state, word] of words) expect(etaText(download({ state })), state).toBe(word)
})

test('each state belongs to one filter, and the counts add up', () => {
  const states: DownloadState[] = ['starting', 'waiting', 'downloading', 'stopped', 'done', 'failed', 'downloading']
  expect(states.map(filterOf)).toEqual(['active', 'active', 'active', 'stopped', 'done', 'failed', 'active'])
  expect(countByFilter(states.map((state, id) => download({ id, state })))).toEqual({
    all: 7,
    active: 4,
    stopped: 1,
    done: 1,
    failed: 1,
  })
})
