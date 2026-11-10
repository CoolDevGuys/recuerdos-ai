import { hashUnit } from "./geometry";

/**
 * How a category looks on the map: a colour *and* a glyph and its label,
 * never colour alone (owner's accessibility rule; plan §5.3).
 *
 * Every colour keeps at least 3:1 against both the light canvas (white)
 * and the dark one (slate-950). The eight built-ins were chosen from
 * Tailwind's mid-tones to stay apart under simulated protanopia,
 * deuteranopia and tritanopia as well as normal vision (CIELAB ΔE ≥ 14
 * for every pair, checked in palette.test.ts). Eight hues at one
 * lightness cannot all stay far apart for every viewer, which is why the
 * glyph and the label carry the meaning and colour only helps.
 */
export interface CategoryStyle {
  color: string;
  glyph: Glyph;
}

export type Glyph = "code" | "heart" | "diamond" | "folder" | "person" | "star" | "bolt" | "link" | "letter";

const BUILT_IN: Record<string, CategoryStyle> = {
  "preference.coding": { color: "#7c3aed", glyph: "code" },
  "preference.personal": { color: "#be123c", glyph: "heart" },
  decision: { color: "#b45309", glyph: "diamond" },
  "fact.project": { color: "#0369a1", glyph: "folder" },
  "fact.person": { color: "#059669", glyph: "person" },
  experience: { color: "#c026d3", glyph: "star" },
  skill: { color: "#3b82f6", glyph: "bolt" },
  reference: { color: "#64748b", glyph: "link" },
};

/** For custom categories, picked by id hash; their glyph is their initial. */
export const CUSTOM_COLORS = ["#65a30d", "#78716c", "#d97706", "#ec4899", "#4d7c0f", "#ef4444"];

/** The built-in categories' styles, for checks and legends. */
export const BUILT_IN_STYLES: Readonly<Record<string, CategoryStyle>> = BUILT_IN;

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
