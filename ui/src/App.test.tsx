import { render, screen } from '@testing-library/react'
import { expect, test } from 'vitest'

import App from './App'

test('the app shows its name and only the tabs that are real', () => {
  render(<App />)
  expect(screen.getByRole('heading', { name: 'BARRAGE' })).toBeInTheDocument()
  expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual(['Links'])
})

test('the one tab is the selected one', () => {
  render(<App />)
  expect(screen.getByRole('tab', { name: 'Links' })).toHaveAttribute('aria-selected', 'true')
})
