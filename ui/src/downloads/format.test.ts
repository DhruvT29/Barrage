import { expect, test } from 'vitest'

import { formatBytes, percent } from './format'

test('sizes are decimal, with one decimal from kB up', () => {
  expect(formatBytes(0)).toBe('0 B')
  expect(formatBytes(999)).toBe('999 B')
  expect(formatBytes(1000)).toBe('1.0 kB')
  expect(formatBytes(470_300_000)).toBe('470.3 MB')
  expect(formatBytes(8_762_927)).toBe('8.8 MB')
  expect(formatBytes(1_200_000_000)).toBe('1.2 GB')
  expect(formatBytes(4_000_000_000)).toBe('4.0 GB')
  expect(formatBytes(6_114_656_256)).toBe('6.1 GB')
})

test('a size that rounds up to 1000 is written in the next unit', () => {
  expect(formatBytes(999_949)).toBe('999.9 kB')
  expect(formatBytes(999_950)).toBe('1.0 MB')
  expect(formatBytes(999_950_000_000)).toBe('1.0 TB')
})

test('percent rounds down, so 100% means every byte is there', () => {
  // 29 / 100 * 100 is 28.999999999999996 in floating point.
  expect(percent(29, 100)).toBe(29)
  expect(percent(1_200_000_000, 4_000_000_000)).toBe(30)
  expect(percent(2_527_000_000, 6_114_656_256)).toBe(41)
  expect(percent(999, 1000)).toBe(99)
  expect(percent(1000, 1000)).toBe(100)
})

test('an empty file is all there: 100%, never NaN', () => {
  expect(percent(0, 0)).toBe(100)
})
