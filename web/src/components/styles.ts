// Shared class lists, so every field and button looks and behaves alike.

export const label = "block text-sm font-medium text-slate-800 dark:text-slate-200";

export const field =
  "mt-1 block w-full rounded-md border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 outline-none " +
  "focus:border-indigo-500 focus:ring-2 focus:ring-indigo-500/30 aria-invalid:border-red-600 " +
  "disabled:cursor-not-allowed disabled:bg-slate-100 disabled:text-slate-600 " +
  "dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100 dark:disabled:bg-slate-900 dark:disabled:text-slate-400";

export const hint = "mt-1 text-xs text-slate-500 dark:text-slate-400";

export const primaryButton =
  "rounded-md bg-indigo-600 px-4 py-2 text-sm font-medium text-white hover:bg-indigo-500 " +
  "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-600 disabled:opacity-60";

export const secondaryButton =
  "rounded-md border border-slate-300 bg-white px-4 py-2 text-sm font-medium text-slate-800 hover:bg-slate-50 " +
  "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-600 disabled:opacity-60 " +
  "dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100 dark:hover:bg-slate-800";

export const dangerButton =
  "rounded-md border border-red-300 bg-white px-4 py-2 text-sm font-medium text-red-700 hover:bg-red-50 " +
  "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-red-600 disabled:opacity-60 " +
  "dark:border-red-800 dark:bg-slate-900 dark:text-red-400 dark:hover:bg-red-950";
