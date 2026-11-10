/**
 * Client-side check of an API key's shape, so a typo is caught on blur
 * rather than spent on a slow (argon2-verified) round trip. The daemon
 * stays the authority: a well-formed key can still be wrong or revoked.
 *
 * Format (src/identity/domain/api_key.rs): `ra_live_` + 8 hex prefix +
 * 32 hex secret.
 */
const SCHEME = "ra_live_";
const BODY = /^[0-9a-fA-F]{40}$/;

export type KeyProblem = "empty" | "scheme" | "shape";

export function checkApiKey(raw: string): KeyProblem | null {
  const key = raw.trim();
  if (key === "") return "empty";
  if (!key.startsWith(SCHEME)) return "scheme";
  if (!BODY.test(key.slice(SCHEME.length))) return "shape";
  return null;
}

export function describeKeyProblem(problem: KeyProblem): string {
  switch (problem) {
    case "empty":
      return "Enter an API key.";
    case "scheme":
      return "API keys start with “ra_live_”. Check that the whole key was pasted.";
    case "shape":
      return "This key is the wrong length or has characters a key never contains. Check that it was pasted in full.";
  }
}
