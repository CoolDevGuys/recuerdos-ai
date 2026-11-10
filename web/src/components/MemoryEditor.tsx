import { useState, type FormEvent } from "react";
import type { Category, Memory } from "../api/types";
import { FIELDS, validate, validateField, valuesOf, type FormErrors, type FormValues } from "../domain/memoryForm";
import type { MemoryActions } from "../state/memories";
import { MemoryFieldset } from "./MemoryFieldset";
import * as styles from "./styles";

const dateTime = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

/**
 * View or correct one memory. Edits are conditional on the version the
 * editor opened (`If-Match`): if an agent or another tab changed the memory
 * meanwhile, the save is refused and the user chooses — the typed text is
 * never thrown away without asking.
 */
export function MemoryEditor({
  memory,
  categories,
  canWrite,
  actions,
  onSaved,
  onForgotten,
}: {
  memory: Memory;
  categories: readonly Category[];
  canWrite: boolean;
  actions: MemoryActions;
  onSaved: (memory: Memory) => void;
  onForgotten: (memory: Memory) => void;
}) {
  const [base, setBase] = useState(memory);
  const [values, setValues] = useState<FormValues>(() => valuesOf(memory));
  const [errors, setErrors] = useState<FormErrors>({});
  // Which request is in flight, so the buttons say what is happening.
  const [busy, setBusy] = useState<"save" | "forget" | "load" | null>(null);
  const pending = busy !== null;
  const [conflict, setConflict] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [confirmingForget, setConfirmingForget] = useState(false);

  const change = (field: keyof FormValues, value: string) => {
    setValues((current) => ({ ...current, [field]: value }));
    // Once a field has an error, re-check it as the user fixes it.
    if (errors[field] !== undefined) setErrors((current) => ({ ...current, [field]: validateField(field, { ...values, [field]: value }) }));
  };
  const blur = (field: keyof FormValues) => setErrors((current) => ({ ...current, [field]: validateField(field, values) }));

  const save = async (overwrite: boolean) => {
    const found = validate(values, FIELDS);
    setErrors(found);
    if (Object.keys(found).length > 0) return;
    setBusy("save");
    setFailure(null);
    const outcome = await actions.save(base, values, overwrite);
    setBusy(null);
    switch (outcome.kind) {
      case "saved":
        setConflict(false);
        onSaved(outcome.memory);
        return;
      case "unchanged":
        onSaved(base);
        return;
      case "conflict":
        setConflict(true);
        return;
      case "failed":
        setFailure(outcome.message);
    }
  };

  const loadLatest = async () => {
    setBusy("load");
    const latest = await actions.latest(base.id);
    setBusy(null);
    if (latest.kind === "failed") {
      setFailure(latest.message);
      return;
    }
    setBase(latest.value);
    setValues(valuesOf(latest.value));
    setErrors({});
    setConflict(false);
  };

  const forget = async () => {
    setBusy("forget");
    const outcome = await actions.forget(base.id);
    setBusy(null);
    if (outcome.kind === "failed") {
      setConfirmingForget(false);
      setFailure(outcome.message);
      return;
    }
    onForgotten(base);
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    void save(false);
  };

  return (
    <form onSubmit={submit} noValidate>
      <MemoryFieldset
        values={values}
        errors={errors}
        categories={categories}
        readOnly={!canWrite}
        onChange={change}
        onBlur={blur}
      />

      <dl className="mt-5 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-xs text-slate-600 dark:text-slate-400">
        <dt>Created</dt>
        <dd>{dateTime.format(new Date(base.created_at))}</dd>
        <dt>Updated</dt>
        <dd>{dateTime.format(new Date(base.updated_at))}</dd>
        <dt>Confidence</dt>
        <dd>{Math.round(base.confidence * 100)}%</dd>
      </dl>

      {conflict && (
        <div role="alert" className="mt-5 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-200">
          <p className="font-medium">
            <span aria-hidden="true">⚠ </span>This memory changed elsewhere since you opened it.
          </p>
          <p className="mt-1">Load the latest version (your edits here are discarded), or save yours over it.</p>
          <div className="mt-3 flex flex-wrap gap-2">
            <button type="button" disabled={pending} onClick={() => void loadLatest()} className={styles.secondaryButton}>
              Load the latest
            </button>
            <button type="button" disabled={pending} onClick={() => void save(true)} className={styles.secondaryButton}>
              Save mine anyway
            </button>
          </div>
        </div>
      )}

      {failure !== null && (
        <p role="alert" className="mt-5 flex gap-1.5 text-sm text-red-700 dark:text-red-400">
          <span aria-hidden="true">⚠</span>
          {failure}
        </p>
      )}

      {canWrite && (
        <div className="mt-6 flex flex-wrap items-center justify-between gap-3 border-t border-slate-200 pt-4 dark:border-slate-800">
          {confirmingForget ? (
            <div role="group" aria-label="Confirm forgetting" className="flex flex-wrap items-center gap-2">
              <span className="text-sm text-slate-700 dark:text-slate-300">Forget this memory?</span>
              <button type="button" disabled={pending} onClick={() => void forget()} className={styles.dangerButton}>
                {busy === "forget" ? "Forgetting…" : "Forget"}
              </button>
              <button type="button" disabled={pending} onClick={() => setConfirmingForget(false)} className={styles.secondaryButton}>
                Keep it
              </button>
            </div>
          ) : (
            <button type="button" disabled={pending} onClick={() => setConfirmingForget(true)} className={styles.dangerButton}>
              Forget…
            </button>
          )}
          <button type="submit" disabled={pending || conflict} className={styles.primaryButton}>
            {busy === "save" ? "Saving…" : "Save changes"}
          </button>
        </div>
      )}
    </form>
  );
}
