import type { LayoutSeed } from "../domain/map/layout";

/**
 * The last layout of each map, per user and scope, kept in `localStorage`
 * so reopening the map — even after a reload — does not reshuffle it
 * (plan §5.1). It holds memory ids, category ids and coordinates, never
 * memory text, and is cleared on sign-out.
 *
 * Storage can be missing, full or blocked (private windows, quotas); every
 * access is wrapped, and the map simply lays out cold when it fails.
 */
const PREFIX = "recuerdos.map-layout.v1:";

function keyOf(user: string, scope: string | null): string {
  return `${PREFIX}${user}:${scope ?? "*"}`;
}

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function isPair(value: unknown): value is [number, number] {
  return Array.isArray(value) && value.length >= 2 && Number.isFinite(value[0]) && Number.isFinite(value[1]);
}

/** A stored seed, checked: anything malformed reads as no seed. */
export function readLayoutSeed(user: string, scope: string | null): LayoutSeed | null {
  try {
    const raw = storage()?.getItem(keyOf(user, scope));
    if (raw == null) return null;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return null;
    const { anchors, ring, positions } = parsed as Record<string, unknown>;
    if (typeof ring !== "number" || typeof anchors !== "object" || anchors === null) return null;
    if (typeof positions !== "object" || positions === null) return null;
    if (!Object.values(anchors).every(isPair)) return null;
    if (!Object.values(positions).every((p) => isPair(p) && typeof (p as unknown[])[2] === "string")) return null;
    return parsed as LayoutSeed;
  } catch {
    return null;
  }
}

export function writeLayoutSeed(user: string, scope: string | null, seed: LayoutSeed): void {
  const store = storage();
  if (store === null) return;
  const value = JSON.stringify(seed);
  try {
    store.setItem(keyOf(user, scope), value);
  } catch {
    // Most likely full: drop every stored layout and try once more.
    clearLayoutSeeds();
    try {
      store.setItem(keyOf(user, scope), value);
    } catch {
      // Give up quietly; the next open lays out cold.
    }
  }
}

/** Forget every stored layout (sign-out). */
export function clearLayoutSeeds(): void {
  const store = storage();
  if (store === null) return;
  try {
    const keys: string[] = [];
    for (let i = 0; i < store.length; i += 1) {
      const key = store.key(i);
      if (key?.startsWith(PREFIX)) keys.push(key);
    }
    for (const key of keys) store.removeItem(key);
  } catch {
    // Nothing to do.
  }
}
