import { useId, useState, type FormEvent } from "react";
import type { Category, Memory } from "../api/types";
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
}: {
  categories: readonly Category[];
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

    setPending(true);
    setFailure(null);
    if (useAi) {
      const outcome = await actions.ingest(values, values.category !== "");
      setPending(false);
      if (outcome.kind === "failed") return setFailure(outcome.message);
      onAdded({ kind: "ingested", ...describeIngest(outcome.value), firstId: outcome.value.memoryIds[0] ?? null });
    } else {
      const outcome = await actions.create(values, structureOf(structure));
      setPending(false);
      if (outcome.kind === "failed") return setFailure(outcome.message);
      onAdded({ kind: "saved", memory: outcome.value });
    }
  };

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
