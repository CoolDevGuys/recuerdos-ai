import type { IngestResult } from "../api/types";

/** What to tell the user after the AI pipeline handled a submission. */
export function describeIngest(result: IngestResult): { tone: "success" | "info"; message: string } {
  const plain = result.understanding ? "" : " No AI model is configured, so it was stored exactly as written.";
  switch (result.outcome) {
    case "stored": {
      const n = result.memoryIds.length;
      return { tone: "success", message: `Saved ${n} ${n === 1 ? "memory" : "memories"}.${plain}` };
    }
    case "already_known":
      return { tone: "info", message: "Already known: every part of it matched a memory you have. Nothing new was saved." };
    case "nothing_durable":
      return { tone: "info", message: "Nothing lasting was found in it, so nothing was saved. Turn off AI classification to save it word for word." };
    case "changed_without_storing":
      return { tone: "info", message: "It updated or retired existing memories rather than adding a new one." };
  }
}
