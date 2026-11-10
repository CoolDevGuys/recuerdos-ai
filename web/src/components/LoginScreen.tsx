import { useId, useState, type FormEvent } from "react";
import { checkApiKey, describeKeyProblem } from "../domain/apiKey";

/**
 * Presentation only: the caller decides what signing in means and turns a
 * refusal into a message (`describeSignInError`). Follows the form rules:
 * visible label, validation on blur and on submit, a specific message, and
 * the typed key is never cleared on failure.
 */
export function LoginScreen({
  notice,
  onSubmit,
}: {
  notice: string | null;
  /** Resolves to `null` on success, or the message to show. */
  onSubmit: (key: string) => Promise<string | null>;
}) {
  const inputId = useId();
  const hintId = useId();
  const errorId = useId();
  const [key, setKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const validate = (value: string): boolean => {
    const problem = checkApiKey(value);
    setError(problem === null ? null : describeKeyProblem(problem));
    return problem === null;
  };

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (pending || !validate(key)) return;
    setPending(true);
    const failure = await onSubmit(key);
    setPending(false);
    setError(failure);
  };

  return (
    <main className="flex min-h-screen items-center justify-center bg-slate-50 px-4 dark:bg-slate-950">
      <form
        onSubmit={submit}
        noValidate
        className="w-full max-w-md rounded-xl border border-slate-200 bg-white p-6 shadow-sm dark:border-slate-800 dark:bg-slate-900"
      >
        <h1 className="text-xl font-semibold text-slate-900 dark:text-slate-100">Sign in to Recuerdos</h1>
        <p className="mt-1 text-sm text-slate-600 dark:text-slate-400">
          Your memories, as a map you can explore and correct.
        </p>

        {notice !== null && (
          <p role="status" className="mt-4 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-200">
            {notice}
          </p>
        )}

        <label htmlFor={inputId} className="mt-6 block text-sm font-medium text-slate-800 dark:text-slate-200">
          API key
        </label>
        <input
          id={inputId}
          name="api-key"
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={key}
          onChange={(event) => setKey(event.target.value)}
          onBlur={() => key !== "" && validate(key)}
          aria-invalid={error !== null}
          aria-describedby={error !== null ? `${errorId} ${hintId}` : hintId}
          className="mt-1 block w-full rounded-md border border-slate-300 bg-white px-3 py-2 font-mono text-sm text-slate-900 outline-none focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/30 aria-invalid:border-red-600 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100"
        />
        <p id={hintId} className="mt-1 text-xs text-slate-500 dark:text-slate-400">
          Issue one with <code className="font-mono">recuerdos-ai key issue --user &lt;you&gt;</code>. It is kept
          only in this tab.
        </p>
        {error !== null && (
          <p id={errorId} role="alert" className="mt-2 flex gap-1.5 text-sm text-red-700 dark:text-red-400">
            <span aria-hidden="true">⚠</span>
            {error}
          </p>
        )}

        <button
          type="submit"
          disabled={pending}
          className="mt-6 w-full rounded-md bg-indigo-600 px-4 py-2 text-sm font-medium text-white hover:bg-indigo-500 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-600 disabled:opacity-60"
        >
          {pending ? "Signing in…" : "Sign in"}
        </button>
      </form>
    </main>
  );
}
