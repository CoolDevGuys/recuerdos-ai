import type { Access } from "../domain/access";

export function Header({
  user,
  access,
  keyless,
  onSignOut,
}: {
  user: string;
  access: Access;
  keyless: boolean;
  onSignOut: () => void;
}) {
  return (
    <header className="border-b border-slate-200 bg-white dark:border-slate-800 dark:bg-slate-900">
      {keyless && (
        <p role="status" className="bg-amber-100 px-4 py-1.5 text-center text-xs text-amber-900 dark:bg-amber-950 dark:text-amber-200">
          <span aria-hidden="true">⚠ </span>
          This server runs without authentication: anyone who can reach it is this user.
        </p>
      )}
      <div className="mx-auto flex max-w-6xl flex-wrap items-center justify-between gap-x-4 gap-y-2 px-4 py-3">
        <span className="text-base font-semibold text-slate-900 dark:text-slate-100">Recuerdos</span>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
          <span className="text-slate-700 dark:text-slate-300">{user}</span>
          <span
            className={
              "whitespace-nowrap rounded-full border px-2 py-0.5 text-xs font-medium " +
              (access.canWrite
                ? "border-indigo-300 text-indigo-800 dark:border-indigo-700 dark:text-indigo-300"
                : "border-slate-300 text-slate-700 dark:border-slate-600 dark:text-slate-300")
            }
          >
            {access.canWrite ? "✎ " : "👁 "}
            {access.label}
          </span>
          {!keyless && (
            <button
              type="button"
              onClick={onSignOut}
              className="whitespace-nowrap rounded-md px-2 py-1 text-slate-600 hover:bg-slate-100 hover:text-slate-900 dark:text-slate-400 dark:hover:bg-slate-800 dark:hover:text-slate-100"
            >
              Sign out
            </button>
          )}
        </div>
      </div>
    </header>
  );
}
