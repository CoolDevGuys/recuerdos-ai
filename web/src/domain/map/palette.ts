/**
 * How things look on the map (ai-specs/memory-structure-plan.md D13: group by
 * topic, colour by kind). A memory is coloured by its **kind** and carries the
 * kind's glyph; a topic — the group a memory sits in — is a neutral disc with
 * the topic's initial, so colour means one thing only. Never colour alone:
 * the glyph and the label carry the meaning too (owner's accessibility rule).
 *
 * The eleven kind colours were found by search (maximin over a 16-step RGB
 * grid): every one keeps at least 3:1 against both the light canvas (white)
 * and the dark one (slate-950), and every pair stays at CIELAB ΔE ≥ 18 under
 * normal vision and simulated protanopia, deuteranopia and tritanopia (the
 * test holds them to 14). The plan feared eleven could not all stay that far
 * apart; they can.
 */
export interface CategoryStyle {
  color: string;
  glyph: Glyph;
}

export type Glyph =
  | "code"
  | "heart"
  | "diamond"
  | "folder"
  | "person"
  | "star"
  | "bolt"
  | "link"
  | "letter"
  | "ban"
  | "check"
  | "info"
  | "list"
  | "flag"
  | "calendar";

const KIND: Record<string, CategoryStyle> = {
  preference: { color: "#9900ff", glyph: "heart" },
  rule: { color: "#aa3300", glyph: "ban" },
  convention: { color: "#dd7711", glyph: "check" },
  decision: { color: "#bb8855", glyph: "diamond" },
  fact: { color: "#4499cc", glyph: "info" },
  experience: { color: "#ff22bb", glyph: "star" },
  learning: { color: "#885577", glyph: "bolt" },
  procedure: { color: "#556644", glyph: "list" },
  reference: { color: "#779988", glyph: "link" },
  goal: { color: "#6644cc", glyph: "flag" },
  plan: { color: "#9977ff", glyph: "calendar" },
};

/** Every kind's style, in the order a legend lists them. */
export const KIND_STYLES: Readonly<Record<string, CategoryStyle>> = KIND;

/** A topic group's colour: neutral, so colour on the map always means a kind. */
export const TOPIC_COLOR = "#64748b";

/** How a memory of `kind` looks; a kind this UI does not know is neutral. */
export function kindStyle(kind: string): CategoryStyle {
  return KIND[kind] ?? { color: TOPIC_COLOR, glyph: "letter" };
}

/** How a topic group looks: neutral, with the topic's initial. */
export function categoryStyle(_topic: string): CategoryStyle {
  return { color: TOPIC_COLOR, glyph: "letter" };
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
  ban: '<circle cx="12" cy="12" r="8" fill="none" stroke="#fff" stroke-width="2.6"/><path d="M6.5 6.5l11 11" stroke="#fff" stroke-width="2.6"/>',
  check: '<path d="m5 12.5 4.5 4.5L19 7.5" fill="none" stroke="#fff" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round"/>',
  info: '<circle cx="12" cy="6.5" r="1.9" fill="#fff"/><path d="M12 10.5v8" stroke="#fff" stroke-width="3" stroke-linecap="round"/>',
  list: '<path d="M5 7h14M5 12h14M5 17h14" stroke="#fff" stroke-width="2.6" stroke-linecap="round"/>',
  flag: '<path d="M6 21V4M6 4h11l-2.5 4L17 12H6" fill="#fff" stroke="#fff" stroke-width="2" stroke-linejoin="round"/>',
  calendar: '<rect x="4" y="6" width="16" height="14" rx="2" fill="none" stroke="#fff" stroke-width="2.4"/><path d="M4 10h16M8 3v5M16 3v5" stroke="#fff" stroke-width="2.4" stroke-linecap="round"/>',
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
