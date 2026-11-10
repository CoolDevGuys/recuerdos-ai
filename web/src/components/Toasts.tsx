import { useToasts } from "../state/toasts";

const TONE = {
  success: { icon: "✓", className: "border-emerald-300 dark:border-emerald-700" },
  info: { icon: "ℹ", className: "border-slate-300 dark:border-slate-600" },
  error: { icon: "⚠", className: "border-red-300 dark:border-red-700" },
} as const;

export function Toasts() {
  const { toasts, dismiss } = useToasts();
  return (
    <div aria-live="polite" className="pointer-events-none fixed inset-x-0 bottom-4 z-30 flex flex-col items-center gap-2 px-4">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          role="status"
          className={`pointer-events-auto flex w-full max-w-md items-center gap-3 rounded-lg border bg-white px-4 py-3 text-sm text-slate-900 shadow-lg dark:bg-slate-900 dark:text-slate-100 ${TONE[toast.tone].className}`}
        >
          <span aria-hidden="true">{TONE[toast.tone].icon}</span>
          <span className="flex-1">{toast.message}</span>
          {toast.action !== undefined && (
            <button
              type="button"
              onClick={() => {
                toast.action?.run();
                dismiss(toast.id);
              }}
              className="font-medium text-indigo-700 underline hover:text-indigo-500 dark:text-indigo-300"
            >
              {toast.action.label}
            </button>
          )}
          <button type="button" aria-label="Dismiss" onClick={() => dismiss(toast.id)} className="text-slate-500 hover:text-slate-900 dark:hover:text-slate-100">
            ✕
          </button>
        </div>
      ))}
    </div>
  );
}
