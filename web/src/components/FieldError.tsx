/** A field's error: icon + text, never colour alone. */
export function FieldError({ id, message }: { id: string; message: string | undefined }) {
  if (message === undefined) return null;
  return (
    <p id={id} className="mt-1 flex gap-1.5 text-sm text-red-700 dark:text-red-400">
      <span aria-hidden="true">⚠</span>
      {message}
    </p>
  );
}
