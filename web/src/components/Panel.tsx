import { useEffect, useId, useRef, type ReactNode } from "react";

/**
 * A side panel over the list: full width on a phone, a column on the right
 * otherwise. Escape and the backdrop close it; focus moves into it on open
 * and back to where it was on close.
 */
export function Panel({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  const titleId = useId();
  const body = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    body.current?.querySelector<HTMLElement>("textarea, input, select, button")?.focus();
    return () => previous?.focus();
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="fixed inset-0 z-20 flex justify-end">
      <div className="absolute inset-0 bg-slate-900/40" aria-hidden="true" onClick={onClose} />
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="relative flex h-full w-full max-w-lg flex-col overflow-y-auto bg-white shadow-xl dark:bg-slate-900"
      >
        <div className="flex items-center justify-between border-b border-slate-200 px-5 py-3 dark:border-slate-800">
          <h2 id={titleId} className="text-base font-semibold text-slate-900 dark:text-slate-100">
            {title}
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="rounded-md px-2 py-1 text-slate-500 hover:bg-slate-100 hover:text-slate-900 dark:hover:bg-slate-800 dark:hover:text-slate-100"
          >
            ✕
          </button>
        </div>
        <div ref={body} className="flex-1 px-5 py-4">
          {children}
        </div>
      </div>
    </div>
  );
}
