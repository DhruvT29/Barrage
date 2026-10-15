import type { Status } from '../api/useLive'

const WORDS: Record<Status, string> = {
  live: 'live',
  connecting: 'connecting',
  disconnected: 'disconnected, retrying',
}

/** The page's own connection to the engine: a colour, a shape and a word. */
export function ConnectionLight({ status }: { status: Status }) {
  return (
    <span className={`light ${status}`} role="status" data-testid="light">
      {WORDS[status]}
    </span>
  )
}
