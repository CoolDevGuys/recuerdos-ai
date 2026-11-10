import { hashUnit } from "./geometry";

/**
 * How a category looks on the map: a colour *and* a glyph and its label,
 * never colour alone (owner's accessibility rule; plan §5.3). Colours are
 * mid-tones that keep ≥ 3:1 against both the light and the dark canvas.
 */
export interface CategoryStyle {
  color: string;
  glyph: Glyph;
}

export type Glyph = "code" | "heart" | "diamond" | "folder" | "person" | "star" | "bolt" | "link" | "letter";

const BUILT_IN: Record<string, CategoryStyle> = {
  "preference.coding": { color: "#6366f1", glyph: "code" },
  "preference.personal": { color: "#db2777", glyph: "heart" },
  decision: { color: "#d97706", glyph: "diamond" },
  "fact.project": { color: "#0891b2", glyph: "folder" },
  "fact.person": { color: "#16a34a", glyph: "person" },
  experience: { color: "#8b5cf6", glyph: "star" },
  skill: { color: "#ea580c", glyph: "bolt" },
  reference: { color: "#64748b", glyph: "link" },
};

const CUSTOM_COLORS = ["#0d9488", "#e11d48", "#65a30d", "#a855f7", "#ca8a04", "#2563eb"];

export function categoryStyle(id: string): CategoryStyle {
  const builtIn = BUILT_IN[id];
  if (builtIn !== undefined) return builtIn;
  const color = CUSTOM_COLORS[Math.floor(hashUnit(id) * CUSTOM_COLORS.length)] ?? CUSTOM_COLORS[0]!;
  return { color, glyph: "letter" };
}

const PATHS: Record<Exclude<Glyph, "letter">, string> = {
  code: '<path d="M8 7 3 12l5 5M16 7l5 5-5 5" fill="none" stroke="#fff" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/>',
  heart: '<path d="M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z" fill="#fff"/>',
  diamond: '<path d="M12 3 21 12 12 21 3 12z" fill="#fff"/>',
  folder: '<path d="M3 6h7l2 2h9v11H3z" fill="#fff"/>',
  person: '<circle cx="12" cy="8" r="4" fill="#fff"/><path d="M4 21c0-4.4 3.6-7 8-7s8 2.6 8 7z" fill="#fff"/>',
  star: '<path d="m12 2 3 7 7 .6-5.3 4.7L18.3 22 12 18l-6.3 4 1.6-7.7L2 9.6 9 9z" fill="#fff"/>',
  bolt: '<path d="M13 2 4 14h7l-1 8 9-12h-7z" fill="#fff"/>',
  link: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1" fill="none" stroke="#fff" stroke-width="2.4" stroke-linecap="round"/>',
};

/** The glyph as an SVG data URL (white on transparent), for the map's pictogram nodes. */
export function glyphImage(glyph: Glyph, label: string): string {
  const body =
    glyph === "letter"
      ? `<text x="12" y="17" text-anchor="middle" font-family="system-ui,sans-serif" font-size="15" font-weight="700" fill="#fff">${escapeXml(
          (label.trim()[0] ?? "?").toUpperCase(),
        )}</text>`
      : PATHS[glyph];
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="64" height="64">${body}</svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

function escapeXml(text: string): string {
  return text.replace(/[<>&"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

/** `#rrggbb` + alpha → `rgba(…)`. */
export function withAlpha(hex: string, alpha: number): string {
  const n = Number.parseInt(hex.slice(1), 16);
  const a = Math.round(Math.min(1, Math.max(0, alpha)) * 1000) / 1000;
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`;
}
