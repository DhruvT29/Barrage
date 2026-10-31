import { expect, test } from 'vitest'

import { formatBytes, formatEta, percent, percentText } from './format'

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

test('the percent shown on a card has one decimal, rounded down, and "-" without a length', () => {
  expect(percentText(16_600_000_000, 34_200_000_000)).toBe('48.5%')
  expect(percentText(1_200_000_000, 4_000_000_000)).toBe('30.0%')
  // 99.99% is not done: it reads 99.9%, never 100.0%.
  expect(percentText(9_999, 10_000)).toBe('99.9%')
  expect(percentText(10_000, 10_000)).toBe('100.0%')
  expect(percentText(0, 10_000)).toBe('0.0%')
  expect(percentText(0, 0)).toBe('100.0%')
  expect(percentText(12_300_000, null)).toBe('-')
})

test('a time left reads as the two largest units, seconds padded', () => {
  expect(formatEta(0)).toBe('0s')
  expect(formatEta(42)).toBe('42s')
  expect(formatEta(65)).toBe('1m 05s')
  expect(formatEta(712)).toBe('11m 52s')
  expect(formatEta(3_600)).toBe('1h 00m')
  expect(formatEta(3_840)).toBe('1h 04m')
  expect(formatEta(90_061)).toBe('1d 1h')
})

test('a time left is rounded up to the whole second: 0.2 s left is not 0s', () => {
  expect(formatEta(0.2)).toBe('1s')
  expect(formatEta(59.5)).toBe('1m 00s')
})

test('a percent never passes 100, as the card\'s percent text does not: a bar or a progress value can never overrun', () => {
  expect(percent(1100, 1000)).toBe(100)
  expect(percentText(1100, 1000)).toBe('100.0%')
})
