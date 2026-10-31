// The cables' icons, drawn here: simple 1.5px strokes in the current colour.
// Decorative only; the name beside every icon says what it is.
import type { InterfaceKind } from './cables'

const STROKE = {
  fill: 'none',
  stroke: 'currentColor',
  strokeWidth: 1.5,
  strokeLinecap: 'round',
  strokeLinejoin: 'round',
} as const

/** Wi-Fi, Bluetooth or a cable, by the interface's name (cables.ts's `interfaceKind`). */
export function InterfaceIcon({ kind, size = 20 }: { kind: InterfaceKind; size?: number }) {
  return (
    <svg viewBox="0 0 24 24" width={size} height={size} aria-hidden="true" data-testid="icon" data-kind={kind} {...STROKE}>
      {kind === 'wifi' && (
        <>
          <path d="M2.5 9a14 14 0 0 1 19 0" />
          <path d="M5.5 12.5a9.5 9.5 0 0 1 13 0" />
          <path d="M8.6 15.9a5 5 0 0 1 6.8 0" />
          <circle cx="12" cy="19" r="0.9" fill="currentColor" />
        </>
      )}
      {kind === 'bluetooth' && <path d="M7 7.5 17 16.5 12 21V3l5 4.5L7 16.5" />}
      {kind === 'cable' && (
        <>
          {/* An Ethernet plug, from the front: the body, its latch and pins, the lead. */}
          <path d="M6 4h12v9.5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 6 13.5Z" />
          <path d="M9.5 4v3M12 4v3M14.5 4v3" />
          <path d="M10 15v2.5h4V15" />
          <path d="M12 17.5V21" />
        </>
      )}
    </svg>
  )
}

/** A meter's dial: the mark that goes with the word "metered", never instead of it. */
export function MeteredMark() {
  return (
    <svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true" {...STROKE}>
      <path d="M1.5 9a4.5 4.5 0 1 1 9 0" />
      <path d="M6 9 8.2 5.6" />
    </svg>
  )
}

/** A lightning bolt for the fastest cable's badge. */
export function BoltMark() {
  return (
    <svg viewBox="0 0 12 12" width="9" height="9" aria-hidden="true" {...STROKE}>
      <path d="M6.8 1 2.5 7h3.2l-.5 4L9.5 5H6.3Z" />
    </svg>
  )
}
