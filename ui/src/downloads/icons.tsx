// The transfer queue's icons: simple 1.5 px strokes in the text's colour,
// drawn on a 16 px grid. Each is decoration: the button or label beside it
// says the same in words.
import type { ReactNode } from 'react'

function Icon({ children, size = 14 }: { children: ReactNode; size?: number }) {
  return (
    <svg
      className="tq-icon"
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  )
}

/** Stop: two bars. */
export function PauseIcon() {
  return (
    <Icon>
      <rect x="4" y="3" width="2.5" height="10" rx="0.5" />
      <rect x="9.5" y="3" width="2.5" height="10" rx="0.5" />
    </Icon>
  )
}

/** Resume: a triangle. */
export function PlayIcon() {
  return (
    <Icon>
      <path d="M5 3.2v9.6L12.6 8z" />
    </Icon>
  )
}

/** The toggle of a card's list of cables: it turns over when open. */
export function ChevronIcon() {
  return (
    <Icon size={12}>
      <path d="M4 6l4 4 4-4" />
    </Icon>
  )
}

/** Fetched: a drive. */
export function DriveIcon() {
  return (
    <Icon size={12}>
      <path d="M2 9.5L4 4h8l2 5.5" />
      <rect x="2" y="9.5" width="12" height="3.5" rx="1" />
      <path d="M11 11.25h.01" />
    </Icon>
  )
}

/** Speed: a gauge. */
export function GaugeIcon() {
  return (
    <Icon size={12}>
      <path d="M2.5 12a5.5 5.5 0 1 1 11 0" />
      <path d="M8 12l3-4" />
    </Icon>
  )
}

/** Time left: a clock. */
export function ClockIcon() {
  return (
    <Icon size={12}>
      <circle cx="8" cy="8" r="5.5" />
      <path d="M8 5v3l2 1.5" />
    </Icon>
  )
}

/** Connections: parallel lines meeting at a point. */
export function ConnectionsIcon() {
  return (
    <Icon size={12}>
      <path d="M2 3.5h4l4 4.5h4" />
      <path d="M2 8h12" />
      <path d="M2 12.5h4l4-4.5" />
    </Icon>
  )
}

/** Cables: a plug. */
export function CableIcon() {
  return (
    <Icon size={12}>
      <path d="M6 2v3M10 2v3" />
      <path d="M4.5 5h7v2.5a3.5 3.5 0 0 1-7 0z" />
      <path d="M8 11v3" />
    </Icon>
  )
}

/** The empty queue's picture: a tray with an arrow into it. */
export function TrayIcon() {
  return (
    <Icon size={28}>
      <path d="M8 2v7M5 6.5L8 9.5l3-3" />
      <path d="M2 9.5h3.5l1 2h3l1-2H14V13a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z" />
    </Icon>
  )
}
