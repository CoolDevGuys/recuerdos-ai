import { useId, useState, type FormEvent } from "react";
import type { RecallHit } from "../../api/types";
import { describeStructure, topicLabelOf } from "../../domain/structureForm";
import type { Outcome } from "../../state/memories";
import { FieldError } from "../FieldError";
import * as styles from "../styles";

function legs(hit: RecallHit): string {
  const found: string[] = [];
  if (hit.matched.vector !== null) found.push(`meaning #${hit.matched.vector}`);
  if (hit.matched.keyword !== null) found.push(`words #${hit.matched.keyword}`);
  if (hit.matched.graph !== null) found.push(`graph #${hit.matched.graph}`);
  if (hit.matched.rerank !== null) found.push(`rerank #${hit.matched.rerank}`);
  return found.join(" · ");
}

/**
 * Ask a question the way an agent would, and see what recall returns and
 * why (plan §5.4, G5). The hits light up on the map.
 */
export function RecallPanel({
  recall,
  onResults,
  onPick,
  labelOf = topicLabelOf,
}: {
  recall: (query: string) => Promise<Outcome<RecallHit[]>>;
  /** The hits to highlight, or `null` to clear. */
  onResults: (hits: RecallHit[] | null) => void;
  onPick: (hit: RecallHit) => void;
  /** A topic's words. */
  labelOf?: (id: string) => string;
}) {
  const inputId = useId();
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | undefined>(undefined);
  const [failure, setFailure] = useState<string | null>(null);
  const [hits, setHits] = useState<RecallHit[] | null>(null);
  const [pending, setPending] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (query.trim() === "") {
      setError("Type a question, such as “which package manager do I prefer?”");
      return;
    }
    setError(undefined);
    setFailure(null);
    setPending(true);
    const outcome = await recall(query.trim());
    setPending(false);
    if (outcome.kind === "failed") {
      setFailure(outcome.message);
      return;
    }
    setHits(outcome.value);
    onResults(outcome.value);
  };

  const clear = () => {
    setHits(null);
    setQuery("");
    onResults(null);
  };

  return (
    <section aria-labelledby={`${inputId}-heading`}>
      <h2 id={`${inputId}-heading`} className="text-sm font-semibold text-slate-900 dark:text-slate-100">
        Recall preview
      </h2>
      <form onSubmit={submit} noValidate className="mt-2">
        <label htmlFor={inputId} className={styles.label}>
          Ask your memories
        </label>
        <div className="mt-1 flex gap-2">
          <input
            id={inputId}
            type="search"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            aria-invalid={error !== undefined}
            aria-describedby={error !== undefined ? `${inputId}-error` : undefined}
            className={`${styles.field} mt-0`}
          />
          <button type="submit" disabled={pending} className={styles.primaryButton}>
            {pending ? "…" : "Recall"}
          </button>
        </div>
        <FieldError id={`${inputId}-error`} message={error} />
      </form>

      {failure !== null && (
        <p role="alert" className="mt-2 text-sm text-red-700 dark:text-red-400">
          <span aria-hidden="true">⚠ </span>
          {failure}
        </p>
      )}

      {hits !== null && (
        <div className="mt-3">
          <div className="flex items-center justify-between">
            <p role="status" className="text-xs text-slate-600 dark:text-slate-400">
              {hits.length === 0 ? "Recall returns nothing for this." : `${hits.length} returned, best first`}
            </p>
            <button type="button" onClick={clear} className="text-xs font-medium text-indigo-700 underline dark:text-indigo-300">
              Clear
            </button>
          </div>
          <ol className="mt-2 space-y-1">
            {hits.map((hit, index) => (
              <li key={hit.memory.id}>
                <button
                  type="button"
                  onClick={() => onPick(hit)}
                  className="block w-full rounded-md border border-slate-200 px-3 py-2 text-left hover:bg-slate-50 dark:border-slate-700 dark:hover:bg-slate-800"
                >
                  <span className="text-sm text-slate-900 dark:text-slate-100">
                    {index + 1}. {hit.memory.content}
                  </span>
                  <span className="mt-0.5 block text-xs text-slate-600 dark:text-slate-400">
                    {Math.round(hit.relevance * 100)}% relevant · {describeStructure(hit.memory, labelOf)}
                    {legs(hit) !== "" && ` · ${legs(hit)}`}
                  </span>
                </button>
              </li>
            ))}
          </ol>
        </div>
      )}
    </section>
  );
}
