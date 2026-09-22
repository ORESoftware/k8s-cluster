// Per-render id generator for components that can appear more than once on a
// page and need a DOM id (SVG gradients, filters, `aria-labelledby` targets).
//
// Duplicate ids are invalid HTML and silently break `url(#…)` references: every
// reference resolves to the first matching element, so the second instance
// inherits the first one's paint. Module state persists for the whole build, so
// ids stay unique and deterministic for a given render order.
const counters = new Map<string, number>();

export function uniqueId(prefix: string): string {
  const next = (counters.get(prefix) ?? 0) + 1;
  counters.set(prefix, next);
  return `${prefix}-${next}`;
}
