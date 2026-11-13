import { useState, type FormEvent } from "react";
import type { Category, GoalStatus, Memory } from "../api/types";
import { validate, validateField, valuesOf, type FormErrors, type FormValues } from "../domain/memoryForm";
import {
  structurePatchOf,
  structureValuesOf,
  validateStructure,
  type StructureErrors,
  type StructureValues,
} from "../domain/structureForm";
import type { MemoryActions } from "../state/memories";
import { MemoryFieldset } from "./MemoryFieldset";
import { StructureFieldset } from "./StructureFieldset";
import * as styles from "./styles";

/** The fields a memory's text is made of; its category follows its structure. */
const TEXT_FIELDS: readonly (keyof FormValues)[] = ["content", "subcategory", "tags"];

const GOAL_STATUS: Record<GoalStatus, string> = {
  active: "Open: you are working towards it.",
  achieved: "Achieved. It has left recall and the profile, and stays in your history.",
  abandoned: "Abandoned. It has left recall and the profile, and stays in your history.",
};

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
  topics,
  projects = [],
}: {
  memory: Memory;
  categories: readonly Category[];
  canWrite: boolean;
  actions: MemoryActions;
  onSaved: (memory: Memory) => void;
  onForgotten: (memory: Memory) => void;
  topics?: readonly { id: string; label: string }[];
  projects?: readonly string[];
}) {
  const [base, setBase] = useState(memory);
  const [values, setValues] = useState<FormValues>(() => valuesOf(memory));
  const [errors, setErrors] = useState<FormErrors>({});
  const [structure, setStructure] = useState<StructureValues>(() => structureValuesOf(memory));
  const [structureErrors, setStructureErrors] = useState<StructureErrors>({});
  // Which request is in flight, so the buttons say what is happening.
  const [busy, setBusy] = useState<"save" | "forget" | "load" | "goal" | "review" | null>(null);
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

  const changeStructure = (field: keyof StructureValues, value: string) => {
    const next = { ...structure, [field]: value } as StructureValues;
    setStructure(next);
    if (Object.keys(structureErrors).length > 0) setStructureErrors(validateStructure(next));
  };

  const save = async (overwrite: boolean) => {
    const found = validate(values, TEXT_FIELDS);
    const structureFound = validateStructure(structure);
    setErrors(found);
    setStructureErrors(structureFound);
    if (Object.keys(found).length > 0 || Object.keys(structureFound).length > 0) return;
    setBusy("save");
    setFailure(null);
    const outcome = await actions.save(base, values, overwrite, structurePatchOf(base, structure));
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
    setStructure(structureValuesOf(latest.value));
    setErrors({});
    setStructureErrors({});
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

  const setGoal = async (status: GoalStatus) => {
    setBusy("goal");
    setFailure(null);
    const outcome = await actions.setGoalStatus(base, status);
    setBusy(null);
    if (outcome.kind === "saved") onSaved(outcome.memory);
    else if (outcome.kind === "conflict") setConflict(true);
    else if (outcome.kind === "failed") setFailure(outcome.message);
  };

  /** "It is right as it is": the memory leaves the review queue. */
  const confirmGuess = async () => {
    setBusy("review");
    setFailure(null);
    const outcome = await actions.bulkEdit([base.id], { reviewed: true });
    setBusy(null);
    if (outcome.kind === "failed") return setFailure(outcome.message);
    const reviewed = outcome.value.memories[0];
    if (reviewed !== undefined) onSaved(reviewed);
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    void save(false);
  };

  return (
    <form onSubmit={submit} noValidate>
      {base.needs_review && (
        <div className="mb-4 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-200">
          <p className="font-medium">Filed by a guess</p>
          <p className="mt-1">
            This memory was saved before memories said what they are and who they are for, so its structure below was
            guessed. Correct it and save, or confirm it if it is right.
          </p>
          {canWrite && (
            <button
              type="button"
              disabled={pending}
              onClick={() => void confirmGuess()}
              className={`mt-2 ${styles.secondaryButton}`}
            >
              {busy === "review" ? "Confirming…" : "It is right as it is"}
            </button>
          )}
        </div>
      )}
      <MemoryFieldset
        values={values}
        errors={errors}
        categories={categories}
        readOnly={!canWrite}
        showCategory={false}
        onChange={change}
        onBlur={blur}
      />
      <StructureFieldset
        values={structure}
        errors={structureErrors}
        readOnly={!canWrite}
        onChange={changeStructure}
        {...(topics !== undefined && { topics })}
        projects={projects}
      />

      {base.kind === "goal" && base.goal_status !== null && (
        <section aria-label="Goal" className="mt-5 rounded-md border border-slate-200 p-3 dark:border-slate-700">
          <p className="text-sm text-slate-800 dark:text-slate-200">
            <span className="font-medium">Goal: </span>
            {GOAL_STATUS[base.goal_status]}
          </p>
          {canWrite && (
            <div className="mt-2 flex flex-wrap gap-2">
              {base.goal_status === "active" ? (
                <>
                  <button type="button" disabled={pending} onClick={() => void setGoal("achieved")} className={styles.secondaryButton}>
                    Mark achieved
                  </button>
                  <button type="button" disabled={pending} onClick={() => void setGoal("abandoned")} className={styles.secondaryButton}>
                    Abandon it
                  </button>
                </>
              ) : (
                <button type="button" disabled={pending} onClick={() => void setGoal("active")} className={styles.secondaryButton}>
                  Reopen it
                </button>
              )}
            </div>
          )}
        </section>
      )}

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
