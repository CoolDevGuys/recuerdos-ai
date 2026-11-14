import { useId, useMemo, useState } from "react";
import type { Memory, Suggestion } from "../api/types";
import { memoriesCount } from "../domain/categories";
import { describeStructure, kindOf, topicLabelOf } from "../domain/structureForm";
import type { MemoryActions } from "../state/memories";
import { useToasts } from "../state/toasts";
import { BulkEditor } from "./BulkEditor";
import { primaryButton, secondaryButton } from "./styles";

/** Rows shown at once; the rest wait their turn. */
const SHOWN = 50;
/** The daemon's limit on one suggestion call. */
const SUGGEST_BATCH = 25;

/**
 * The Needs review queue (ai-specs/memory-structure-plan.md §8): memories
 * saved before memories had a structure, filed by a guess the user should
 * check. Select some and file them with the bulk editor, or — with a model —
 * ask for **Suggest**ions and accept them one by one or all at once. Nothing
 * is rewritten without the user's click.
 */
export function ReviewQueue({
  memories,
  topics,
  projects = [],
  canWrite,
  canSuggest,
  actions,
  onOpen,
}: {
  memories: readonly Memory[];
  topics?: readonly { id: string; label: string }[];
  projects?: readonly string[];
  canWrite: boolean;
  /** A model is configured, so Suggest works. */
  canSuggest: boolean;
  actions: MemoryActions;
  onOpen: (memory: Memory) => void;
}) {
  const id = useId();
  const { show } = useToasts();
  const queue = useMemo(() => memories.filter((m) => m.needs_review), [memories]);
  const shown = queue.slice(0, SHOWN);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [suggestions, setSuggestions] = useState<ReadonlyMap<string, Suggestion>>(new Map());
  const [busy, setBusy] = useState<"suggest" | "accept" | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const topicLabel = useMemo(() => {
    const labels = new Map((topics ?? []).map((t) => [t.id, t.label]));
    return (topic: string) => labels.get(topic) ?? topicLabelOf(topic);
  }, [topics]);

  // Memories that left the queue (accepted, edited) take their state with them.
  const present = new Set(queue.map((m) => m.id));
  const chosen = [...selected].filter((m) => present.has(m));
  const pending = [...suggestions.values()].filter((s) => present.has(s.id));

  const toggle = (ids: readonly string[], on: boolean) =>
    setSelected((current) => {
      const next = new Set(current);
      for (const memory of ids) {
        if (on) next.add(memory);
        else next.delete(memory);
      }
      return next;
    });

  const suggest = async () => {
    const asked = (chosen.length > 0 ? chosen : shown.map((m) => m.id)).slice(0, SUGGEST_BATCH);
    setBusy("suggest");
    setFailure(null);
    const outcome = await actions.suggest(asked);
    setBusy(null);
    if (outcome.kind === "failed") return setFailure(outcome.message);
    setSuggestions((current) => new Map([...current, ...outcome.value.map((s) => [s.id, s] as const)]));
    const missing = asked.length - outcome.value.length;
    show({
      tone: "info",
      message:
        outcome.value.length === 0
          ? "No suggestion came back for these. File them yourself."
          : `${outcome.value.length} suggested${missing > 0 ? `; ${missing} could not be placed` : ""}. Check each before you accept it.`,
    });
  };

  /** Accepts suggestions, one bulk edit per distinct structure. */
  const accept = async (accepted: readonly Suggestion[]) => {
    const groups = new Map<string, Suggestion[]>();
    for (const s of accepted) {
      const key = `${s.kind}|${s.topic}|${s.applies_to.target}|${s.applies_to.project_name ?? ""}`;
      groups.set(key, [...(groups.get(key) ?? []), s]);
    }
    setBusy("accept");
    setFailure(null);
    let filed = 0;
    for (const group of groups.values()) {
      const first = group[0]!;
      const outcome = await actions.bulkEdit(
        group.map((s) => s.id),
        { kind: first.kind, topic: first.topic, appliesTo: first.applies_to },
      );
      if (outcome.kind === "failed") {
        setBusy(null);
        setFailure(outcome.message);
        return;
      }
      filed += group.length;
    }
    setBusy(null);
    setSuggestions((current) => new Map([...current].filter(([memory]) => !accepted.some((s) => s.id === memory))));
    show({ tone: "success", message: `Filed ${memoriesCount(filed)}.` });
  };

  /** Files a memory filed as someone else's as the user's own. */
  const fileAsMine = async (memory: Memory) => {
    setFailure(null);
    const outcome = await actions.bulkEdit([memory.id], { appliesTo: { target: "me", project_name: null } });
    if (outcome.kind === "failed") return setFailure(outcome.message);
    show({
      tone: "success",
      message: memory.kind === "fact" ? "Filed as your personal information." : "Filed as yours.",
    });
  };

  if (queue.length === 0) {
    return (
      <p className="rounded-lg border border-dashed border-slate-300 p-8 text-center text-sm text-slate-600 dark:border-slate-700 dark:text-slate-400">
        Nothing to review: every memory says what it is and who it is for.
      </p>
    );
  }

  return (
    <section aria-labelledby={`${id}-title`} className="space-y-4">
      <div>
        <h2 id={`${id}-title`} className="text-base font-semibold text-slate-900 dark:text-slate-100">
          Needs review · {memoriesCount(queue.length)}
        </h2>
        <p className="mt-1 text-sm text-slate-600 dark:text-slate-400">
          These were saved before memories said what they are and who they are for, so their structure was guessed from
          their old category. Select some and file them below, or open one to edit it.
          {canSuggest && " Suggest asks the language model for a proposal you can accept or change."}
        </p>
      </div>

      {canWrite && (
        <div className="flex flex-wrap gap-2">
          {canSuggest && (
            <button type="button" disabled={busy !== null} onClick={() => void suggest()} className={secondaryButton}>
              {busy === "suggest"
                ? "Asking…"
                : chosen.length > 0
                  ? `Suggest for ${memoriesCount(Math.min(chosen.length, SUGGEST_BATCH))}`
                  : `Suggest for the first ${Math.min(shown.length, SUGGEST_BATCH)}`}
            </button>
          )}
          {pending.length > 1 && (
            <button type="button" disabled={busy !== null} onClick={() => void accept(pending)} className={primaryButton}>
              {busy === "accept" ? "Filing…" : `Accept all ${pending.length} suggestions`}
            </button>
          )}
        </div>
      )}

      {failure !== null && (
        <p role="alert" className="text-sm text-red-700 dark:text-red-400">
          <span aria-hidden="true">⚠ </span>
          {failure}
        </p>
      )}

      {canWrite && chosen.length > 0 && (
        <BulkEditor
          selected={chosen}
          {...(topics !== undefined && { topics })}
          projects={projects}
          actions={actions}
          onDone={() => setSelected(new Set())}
          onClear={() => setSelected(new Set())}
        />
      )}

      {canWrite && (
        <label className="flex items-center gap-2 text-sm text-slate-800 dark:text-slate-200">
          <input
            type="checkbox"
            checked={shown.every((m) => selected.has(m.id))}
            onChange={(event) => toggle(shown.map((m) => m.id), event.target.checked)}
            className="h-4 w-4 accent-indigo-600"
          />
          Select the {shown.length} shown
        </label>
      )}

      <ul className="divide-y divide-slate-200 rounded-lg border border-slate-200 bg-white dark:divide-slate-800 dark:border-slate-800 dark:bg-slate-900">
        {shown.map((memory) => {
          const suggestion = suggestions.get(memory.id);
          return (
            <li key={memory.id} className="flex gap-3 px-4 py-3">
              {canWrite && (
                <input
                  type="checkbox"
                  aria-label={`Select: ${memory.content}`}
                  checked={selected.has(memory.id)}
                  onChange={(event) => toggle([memory.id], event.target.checked)}
                  className="mt-1 h-4 w-4 shrink-0 accent-indigo-600"
                />
              )}
              <div className="min-w-0 flex-1">
                <button
                  type="button"
                  onClick={() => onOpen(memory)}
                  className="text-left text-sm text-slate-900 hover:underline dark:text-slate-100"
                >
                  {memory.content}
                </button>
                <p className="mt-1 text-xs text-slate-600 dark:text-slate-400">
                  Guessed: {describeStructure(memory, topicLabel)} · was filed as {memory.category}
                  {canWrite && memory.applies_to.target === "general" && (kindOf(memory.kind)?.targets.includes("me") ?? false) && (
                    <>
                      {" · "}
                      <button
                        type="button"
                        disabled={busy !== null}
                        onClick={() => void fileAsMine(memory)}
                        className="font-medium text-indigo-700 underline dark:text-indigo-300"
                      >
                        It’s about me
                      </button>
                    </>
                  )}
                </p>
                {suggestion !== undefined && (
                  <div className="mt-2 flex flex-wrap items-center gap-2 rounded-md bg-indigo-50 px-3 py-2 text-sm text-indigo-950 dark:bg-indigo-950/50 dark:text-indigo-100">
                    <span>
                      <span className="font-medium">Suggested: </span>
                      {kindOf(suggestion.kind)?.label ?? suggestion.kind} · {topicLabel(suggestion.topic)} ·{" "}
                      {suggestion.applies_to.target === "project"
                        ? suggestion.applies_to.project_name
                        : suggestion.applies_to.target === "me"
                          ? "you"
                          : "general"}
                      {suggestion.reason !== "" && <span className="text-indigo-800 dark:text-indigo-300"> — {suggestion.reason}</span>}
                    </span>
                    {canWrite && (
                      <button
                        type="button"
                        disabled={busy !== null}
                        onClick={() => void accept([suggestion])}
                        className="font-medium text-indigo-700 underline dark:text-indigo-300"
                      >
                        Accept
                      </button>
                    )}
                  </div>
                )}
              </div>
            </li>
          );
        })}
      </ul>
      {queue.length > shown.length && (
        <p className="text-sm text-slate-600 dark:text-slate-400">
          {memoriesCount(queue.length - shown.length)} more wait their turn: file these first.
        </p>
      )}
    </section>
  );
}
