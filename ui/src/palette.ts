// One colour per cable, the same everywhere on the page: its card, its
// sparkline, its band in the hero's stacked bar and chart, its node on the
// radar and its blocks in a download's piece bar. The order is the order
// cables first appear in the link table, so a colour never jumps between
// cards while the engine runs. The values match the `--cable-N` tokens in
// theme.css.
export const CABLE_COLORS = ['#22ff66', '#22d3ee', '#b6f53b', '#ffb020', '#5eead4', '#a78bfa', '#f472b6'] as const

/** The colour of the cable at `index` in the table's order; it wraps after the last. */
export function cableColor(index: number): string {
  return CABLE_COLORS[((index % CABLE_COLORS.length) + CABLE_COLORS.length) % CABLE_COLORS.length]
}

/** The colours by cable name, from the cables' names in table order. */
export function colorsByName(names: readonly string[]): Record<string, string> {
  return Object.fromEntries(names.map((name, i) => [name, cableColor(i)]))
}
