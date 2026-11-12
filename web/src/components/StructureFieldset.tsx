import { useId } from "react";
import { KINDS, TARGETS, TOPICS, kindOf, type StructureErrors, type StructureValues } from "../domain/structureForm";
import { FieldError } from "./FieldError";
import * as styles from "./styles";

/**
 * What a new memory is, what it is about, and who it is true for
 * (ai-specs/memory-structure-plan.md §3). Presentation only.
 */
export function StructureFieldset({
  values,
  errors,
  readOnly,
  onChange,
}: {
  values: StructureValues;
  errors: StructureErrors;
  readOnly: boolean;
  onChange: (field: keyof StructureValues, value: string) => void;
}) {
  const id = useId();
  const kind = kindOf(values.kind);
  const describedBy = (field: keyof StructureValues, extra?: string) =>
    [errors[field] !== undefined ? `${id}-${field}-error` : null, extra].filter(Boolean).join(" ") || undefined;

  return (
    <div className="mt-4 space-y-4">
      <div className="grid gap-4 sm:grid-cols-2">
        <div>
          <label htmlFor={`${id}-kind`} className={styles.label}>
            What it is
          </label>
          <select
            id={`${id}-kind`}
            value={values.kind}
            disabled={readOnly}
            onChange={(event) => onChange("kind", event.target.value)}
            aria-invalid={errors.kind !== undefined}
            aria-describedby={describedBy("kind", `${id}-kind-hint`)}
            className={styles.field}
          >
            <option value="">Choose…</option>
            {KINDS.map((option) => (
              <option key={option.id} value={option.id}>
                {option.label}
              </option>
            ))}
          </select>
          <p id={`${id}-kind-hint`} className={styles.hint}>
            {kind?.hint ?? "A preference, a rule, a fact…"}
          </p>
          <FieldError id={`${id}-kind-error`} message={errors.kind} />
        </div>
        <div>
          <label htmlFor={`${id}-topic`} className={styles.label}>
            About
          </label>
          <select
            id={`${id}-topic`}
            value={values.topic}
            disabled={readOnly}
            onChange={(event) => onChange("topic", event.target.value)}
            aria-invalid={errors.topic !== undefined}
            aria-describedby={describedBy("topic")}
            className={styles.field}
          >
            <option value="">Choose…</option>
            {TOPICS.map((option) => (
              <option key={option.id} value={option.id}>
                {option.label}
              </option>
            ))}
          </select>
          <FieldError id={`${id}-topic-error`} message={errors.topic} />
        </div>
      </div>

      <fieldset aria-describedby={describedBy("target")}>
        <legend className={styles.label}>Applies to</legend>
        <div className="mt-1 space-y-1.5">
          {TARGETS.map((option) => {
            const allowed = kind === undefined || kind.targets.includes(option.id);
            return (
              <label key={option.id} className="flex items-start gap-2 text-sm text-slate-800 dark:text-slate-200">
                <input
                  type="radio"
                  name={`${id}-target`}
                  value={option.id}
                  checked={values.target === option.id}
                  disabled={readOnly || !allowed}
                  onChange={() => onChange("target", option.id)}
                  className="mt-0.5 h-4 w-4 accent-indigo-600"
                />
                <span>
                  {option.label}
                  <span className="block text-xs text-slate-500 dark:text-slate-400">
                    {allowed ? option.hint : `Not for a ${kind?.label.toLowerCase()}.`}
                  </span>
                </span>
              </label>
            );
          })}
        </div>
        <FieldError id={`${id}-target-error`} message={errors.target} />
      </fieldset>

      {values.target === "project" && (
        <div>
          <label htmlFor={`${id}-project`} className={styles.label}>
            Project
          </label>
          <input
            id={`${id}-project`}
            type="text"
            value={values.project}
            readOnly={readOnly}
            onChange={(event) => onChange("project", event.target.value)}
            aria-invalid={errors.project !== undefined}
            aria-describedby={describedBy("project", `${id}-project-hint`)}
            className={styles.field}
          />
          <p id={`${id}-project-hint`} className={styles.hint}>
            Lowercase, like “quill”. A name it does not know yet becomes a new project.
          </p>
          <FieldError id={`${id}-project-error`} message={errors.project} />
        </div>
      )}
    </div>
  );
}
