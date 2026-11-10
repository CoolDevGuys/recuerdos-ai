import type { Scope } from "../api/types";

/** What the signed-in key lets the UI offer. `admin` implies everything. */
export interface Access {
  canRead: boolean;
  canWrite: boolean;
  /** Short label for the header badge. */
  label: string;
}

export function accessFor(scopes: readonly Scope[]): Access {
  const admin = scopes.includes("admin");
  const canRead = admin || scopes.includes("read");
  const canWrite = admin || scopes.includes("write");

  let label: string;
  if (admin) label = "Admin key";
  else if (canRead && canWrite) label = "Read & write key";
  else if (canRead) label = "Read-only key";
  else if (canWrite) label = "Write-only key";
  else label = "No access";

  return { canRead, canWrite, label };
}
