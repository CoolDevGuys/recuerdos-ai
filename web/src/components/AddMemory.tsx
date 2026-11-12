import { useId, useState, type FormEvent } from "react";
import type { Category, ConflictingMemory, Memory } from "../api/types";
import { describeIngest } from "../domain/ingest";
import { emptyValues, validate, validateField, type FormErrors, type FormValues } from "../domain/memoryForm";
import {
  emptyStructure,
  structureOf,
  validateStructure,
  type StructureErrors,
  type StructureValues,
} from "../domain/structureForm";
import type { MemoryActions } from "../state/memories";
import { MemoryFieldset } from "./MemoryFieldset";
import { StructureFieldset } from "./StructureFieldset";
import * as styles from "./styles";

export type Added =
  | { kind: "saved"; memory: Memory }
  /** A stored memory already said it; nothing new was written. */
  | { kind: "known"; memory: Memory }
  | { kind: "ingested"; tone: "success" | "info"; message: string; firstId: string | null };

/**
 * Add a memory, either exactly as written (instant) or through the AI
 * pipeline, which classifies it, splits it and skips what is already known
 * (slower: it waits for the model).
 */
export function AddMemory({
  categories,
  actions,
  onAdded,
  topics,
  projects = [],
}: {
  categories: readonly Category[];
  topics?: readonly { id: string; label: string }[];
  projects?: readonly string[];
  actions: MemoryActions;
  onAdded: (added: Added) => void;
}) {
  const toggleId = useId();
  const [values, setValues] = useState<FormValues>(() => emptyValues("fact.project"));
  const [errors, setErrors] = useState<FormErrors>({});
  const [structure, setStructure] = useState<StructureValues>(emptyStructure);
  const [structureErrors, setStructureErrors] = useState<StructureErrors>({});
  const [useAi, setUseAi] = useState(false);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  // A save refused because it contradicts stored memories: the user decides.
  const [conflict, setConflict] = useState<{ message: string; conflicts: ConflictingMemory[] } | null>(null);

  // With the AI on, only the text is required; the category is a hint.
  // Saved as written, the structure says what it is, and the category follows.
  const checked: (keyof FormValues)[] = useAi ? ["content", "tags"] : ["content", "subcategory", "tags"];

  const change = (field: keyof FormValues, value: string) => {
    setValues((current) => ({ ...current, [field]: value }));
    if (errors[field] !== undefined) setErrors((current) => ({ ...current, [field]: validateField(field, { ...values, [field]: value }) }));
  };
  const blur = (field: keyof FormValues) => {
    if (checked.includes(field)) setErrors((current) => ({ ...current, [field]: validateField(field, values) }));
  };

  const changeStructure = (field: keyof StructureValues, value: string) => {
    const next = { ...structure, [field]: value } as StructureValues;
    setStructure(next);
    if (Object.keys(structureErrors).length > 0) setStructureErrors(validateStructure(next));
  };

  const toggleAi = (on: boolean) => {
    setUseAi(on);
    setErrors({});
    setValues((current) => ({ ...current, category: on ? "" : current.category || "fact.project" }));
  };

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const found = validate(values, checked);
    const structureFound = useAi ? {} : validateStructure(structure);
    setErrors(found);
    setStructureErrors(structureFound);
    if (Object.keys(found).length > 0 || Object.keys(structureFound).length > 0) return;

    if (useAi) {
      setPending(true);
      setFailure(null);
      const outcome = await actions.ingest(values, values.category !== "");
      setPending(false);
      if (outcome.kind === "failed") return setFailure(outcome.message);
      onAdded({ kind: "ingested", ...describeIngest(outcome.value), firstId: outcome.value.memoryIds[0] ?? null });
    } else {
      await saveAsWritten({});
    }
  };

  /** Saves as written, with how to settle a conflict the user chose. */
  const saveAsWritten = async (settle: { replaces?: string; coexists_with?: string[] }) => {
    setPending(true);
    setFailure(null);
    setConflict(null);
    const outcome = await actions.create(values, { ...structureOf(structure), ...settle });
    setPending(false);
    if (outcome.kind === "failed") return setFailure(outcome.message);
    if (outcome.kind === "conflict") return setConflict(outcome);
    const { memory, status } = outcome.value;
    onAdded(status === "already_known" ? { kind: "known", memory } : { kind: "saved", memory });
  };

  // A save replaces one memory; with several in conflict, the user keeps
  // both or edits the stored ones first.
  const only = conflict?.conflicts.length === 1 ? conflict.conflicts[0] : undefined;

  return (
    <form onSubmit={submit} noValidate>
      <MemoryFieldset
        values={values}
        errors={errors}
        categories={categories}
        readOnly={pending}
        categoryOptional={useAi}
        showCategory={useAi}
        onChange={change}
        onBlur={blur}
      />
      {!useAi && (
        <StructureFieldset
          values={structure}
          errors={structureErrors}
          readOnly={pending}
          onChange={changeStructure}
          {...(topics !== undefined && { topics })}
          projects={projects}
        />
      )}

      <div className="mt-5 flex items-start gap-3 rounded-md border border-slate-200 p-3 dark:border-slate-700">
        <input
          id={toggleId}
          type="checkbox"
          checked={useAi}
          disabled={pending}
          onChange={(event) => toggleAi(event.target.checked)}
          aria-describedby={`${toggleId}-hint`}
          className="mt-0.5 h-4 w-4 accent-indigo-600"
        />
        <div>
          <label htmlFor={toggleId} className="text-sm font-medium text-slate-800 dark:text-slate-200">
            Let the AI classify it and skip what is already known
          </label>
          <p id={`${toggleId}-hint`} className={styles.hint}>
            It may split the text into several memories, or save nothing if you already have it. Takes as long as the
            model does.
          </p>
        </div>
      </div>

      {conflict !== null && (
        <div role="alert" className="mt-4 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm dark:border-amber-700 dark:bg-amber-950">
          <p className="font-medium text-amber-900 dark:text-amber-200">
            This may contradict a memory you already have:
          </p>
          <ul className="mt-1 list-disc pl-5 text-amber-900 dark:text-amber-200">
            {conflict.conflicts.map((stored) => (
              <li key={stored.id}>“{stored.content}”</li>
            ))}
          </ul>
          <p className={styles.hint}>
            Replace it if you changed your mind (the old one is kept in history), or keep both if each holds in its
            own case. To fix the stored one instead, edit it from the list.
          </p>
          <div className="mt-3 flex flex-wrap gap-2">
            {only !== undefined && (
              <button
                type="button"
                disabled={pending}
                className={styles.primaryButton}
                onClick={() => void saveAsWritten({ replaces: only.id })}
              >
                Replace it
              </button>
            )}
            <button
              type="button"
              disabled={pending}
              className={styles.secondaryButton}
              onClick={() => void saveAsWritten({ coexists_with: conflict.conflicts.map((stored) => stored.id) })}
            >
              Keep both
            </button>
          </div>
        </div>
      )}

      {failure !== null && (
        <p role="alert" className="mt-4 flex gap-1.5 text-sm text-red-700 dark:text-red-400">
          <span aria-hidden="true">⚠</span>
          {failure}
        </p>
      )}

      <div className="mt-6 flex justify-end border-t border-slate-200 pt-4 dark:border-slate-800">
        <button type="submit" disabled={pending} className={styles.primaryButton}>
          {pending ? (useAi ? "Asking the AI…" : "Saving…") : "Save memory"}
        </button>
      </div>
    </form>
  );
}
