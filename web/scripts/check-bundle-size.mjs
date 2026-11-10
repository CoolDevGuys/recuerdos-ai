// Fails when the gzipped JavaScript + CSS the page loads up front exceeds the
// budget in ai-specs/web-ui-plan.md §8. Run after `pnpm build`.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { gzipSync } from "node:zlib";

const BUDGET_BYTES = 400 * 1024;
const dir = new URL("../dist/assets/", import.meta.url).pathname;

const files = readdirSync(dir).filter((name) => name.endsWith(".js") || name.endsWith(".css"));
let total = 0;
for (const name of files) {
  const size = gzipSync(readFileSync(join(dir, name))).length;
  total += size;
  console.log(`${(size / 1024).toFixed(1).padStart(8)} KB  ${name}`);
}
console.log(`${(total / 1024).toFixed(1).padStart(8)} KB  total (gzip), budget ${BUDGET_BYTES / 1024} KB`);

if (total > BUDGET_BYTES) {
  console.error("Bundle exceeds its size budget.");
  process.exit(1);
}
