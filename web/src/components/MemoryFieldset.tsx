import { useId } from "react";
import type { Category } from "../api/types";
import type { FormErrors, FormValues } from "../domain/memoryForm";
import { FieldError } from "./FieldError";
import * as styles from "./styles";

/**
 * The four fields a memory is made of. Presentation only: the owner keeps
 * the values and errors and decides when to validate.
 */
export function MemoryFieldset({
  values,
  errors,
  categories,
  readOnly,
  categoryOptional = false,
  showCategory = true,
  onChange,
  onBlur,
}: {
  values: FormValues;
  errors: FormErrors;
  categories: readonly Category[];
  readOnly: boolean;
  /** With AI classification the category is only a hint, and may be left to the model. */
  categoryOptional?: boolean;
  /** A structured save derives the category from what the memory is, so it does not ask. */
  showCategory?: boolean;
  onChange: (field: keyof FormValues, value: string) => void;
  onBlur: (field: keyof FormValues) => void;
}) {
  const id = useId();
  const ids = {
    content: `${id}-content`,
    category: `${id}-category`,
    subcategory: `${id}-subcategory`,
    tags: `${id}-tags`,
  };
  const describedBy = (field: keyof FormValues, extra?: string) =>
    [errors[field] !== undefined ? `${ids[field]}-error` : null, extra].filter(Boolean).join(" ") || undefined;

  const known = categories.some((c) => c.id === values.category);

  return (
    <div className="space-y-4">
      <div>
        <label htmlFor={ids.content} className={styles.label}>
          Memory
        </label>
        <textarea
          id={ids.content}
          rows={5}
          value={values.content}
          readOnly={readOnly}
          onChange={(event) => onChange("content", event.target.value)}
          onBlur={() => onBlur("content")}
          aria-invalid={errors.content !== undefined}
          aria-describedby={describedBy("content")}
          className={styles.field}
        />
        <FieldError id={`${ids.content}-error`} message={errors.content} />
      </div>

      {showCategory && (
      <div>
        <label htmlFor={ids.category} className={styles.label}>
          Category{categoryOptional && <span className="font-normal text-slate-500"> (a hint for the AI)</span>}
        </label>
        <select
          id={ids.category}
          value={values.category}
          disabled={readOnly}
          onChange={(event) => onChange("category", event.target.value)}
          onBlur={() => onBlur("category")}
          aria-invalid={errors.category !== undefined}
          aria-describedby={describedBy("category")}
          className={styles.field}
        >
          {categoryOptional && <option value="">Let the AI decide</option>}
          {!known && values.category !== "" && <option value={values.category}>{values.category}</option>}
          {categories.map((category) => (
            <option key={category.id} value={category.id}>
              {category.label}
            </option>
          ))}
        </select>
        <FieldError id={`${ids.category}-error`} message={errors.category} />
      </div>
      )}

      {!categoryOptional && (
        <div>
          <label htmlFor={ids.subcategory} className={styles.label}>
            Sub-label <span className="font-normal text-slate-500">(optional)</span>
          </label>
          <input
            id={ids.subcategory}
            type="text"
            value={values.subcategory}
            readOnly={readOnly}
            onChange={(event) => onChange("subcategory", event.target.value)}
            onBlur={() => onBlur("subcategory")}
            aria-invalid={errors.subcategory !== undefined}
            aria-describedby={describedBy("subcategory", `${ids.subcategory}-hint`)}
            className={styles.field}
          />
          <p id={`${ids.subcategory}-hint`} className={styles.hint}>
            A narrower label, such as “testing”.
          </p>
          <FieldError id={`${ids.subcategory}-error`} message={errors.subcategory} />
        </div>
      )}

      <div>
        <label htmlFor={ids.tags} className={styles.label}>
          Tags <span className="font-normal text-slate-500">(optional)</span>
        </label>
        <input
          id={ids.tags}
          type="text"
          value={values.tags}
          readOnly={readOnly}
          onChange={(event) => onChange("tags", event.target.value)}
          onBlur={() => onBlur("tags")}
          aria-invalid={errors.tags !== undefined}
          aria-describedby={describedBy("tags", `${ids.tags}-hint`)}
          className={styles.field}
        />
        <p id={`${ids.tags}-hint`} className={styles.hint}>
          Separated by commas, such as “pnpm, tooling”.
        </p>
        <FieldError id={`${ids.tags}-error`} message={errors.tags} />
      </div>
    </div>
  );
}
