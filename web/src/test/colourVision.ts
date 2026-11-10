/**
 * Colour-vision checks for the map's palette: WCAG contrast, and CIELAB
 * distance under simulated dichromacy (Machado, Oliveira & Fernandes 2009,
 * severity 1.0, applied in linear RGB).
 */
export type Vision = "normal" | "protanopia" | "deuteranopia" | "tritanopia";

export const VISIONS: readonly Vision[] = ["normal", "protanopia", "deuteranopia", "tritanopia"];

type Matrix = [[number, number, number], [number, number, number], [number, number, number]];

const MACHADO: Record<Exclude<Vision, "normal">, Matrix> = {
  protanopia: [
    [0.152286, 1.052583, -0.204868],
    [0.114503, 0.786281, 0.099216],
    [-0.003882, -0.048116, 1.051998],
  ],
  deuteranopia: [
    [0.367322, 0.860646, -0.227968],
    [0.280085, 0.672501, 0.047413],
    [-0.01182, 0.04294, 0.968881],
  ],
  tritanopia: [
    [1.255528, -0.076749, -0.178779],
    [-0.078411, 0.930809, 0.147602],
    [0.004733, 0.691367, 0.3039],
  ],
};

function linear(hex: string): [number, number, number] {
  const channel = (i: number) => {
    const c = Number.parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return [channel(1), channel(3), channel(5)];
}

function simulate(hex: string, vision: Vision): [number, number, number] {
  const rgb = linear(hex);
  if (vision === "normal") return rgb;
  const clamp = (v: number) => Math.min(1, Math.max(0, v));
  return MACHADO[vision].map((row) => clamp(row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2])) as [number, number, number];
}

function lab([r, g, b]: [number, number, number]): [number, number, number] {
  const x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
  const y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
  const z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
  const f = (t: number) => (t > 0.008856 ? Math.cbrt(t) : 7.787 * t + 16 / 116);
  return [116 * f(y) - 16, 500 * (f(x) - f(y)), 200 * (f(y) - f(z))];
}

/** CIE76 ΔE between two colours as a viewer with `vision` sees them. */
export function distance(a: string, b: string, vision: Vision): number {
  const [p, q] = [lab(simulate(a, vision)), lab(simulate(b, vision))];
  return Math.hypot(p[0] - q[0], p[1] - q[1], p[2] - q[2]);
}

/** WCAG 2 contrast ratio. */
export function contrast(a: string, b: string): number {
  const luminance = (hex: string) => {
    const [r, g, bl] = linear(hex);
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (hi + 0.05) / (lo + 0.05);
}
