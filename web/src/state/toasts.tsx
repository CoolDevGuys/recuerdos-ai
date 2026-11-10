import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";

export interface Toast {
  id: number;
  message: string;
  tone: "success" | "info" | "error";
  action?: { label: string; run: () => void };
}

interface ToastContextValue {
  toasts: Toast[];
  show(toast: Omit<Toast, "id">, durationMs?: number): void;
  dismiss(id: number): void;
}

const ToastContext = createContext<ToastContextValue | null>(null);

/** Short-lived notices; one with an action (Undo) stays its full duration. */
export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const nextId = useRef(1);
  const timers = useRef(new Map<number, ReturnType<typeof setTimeout>>());

  const dismiss = useCallback((id: number) => {
    const timer = timers.current.get(id);
    if (timer !== undefined) clearTimeout(timer);
    timers.current.delete(id);
    setToasts((current) => current.filter((t) => t.id !== id));
  }, []);

  const show = useCallback(
    (toast: Omit<Toast, "id">, durationMs = 5000) => {
      const id = nextId.current++;
      setToasts((current) => [...current.slice(-2), { ...toast, id }]);
      timers.current.set(id, setTimeout(() => dismiss(id), durationMs));
    },
    [dismiss],
  );

  useEffect(() => {
    const pending = timers.current;
    return () => pending.forEach((timer) => clearTimeout(timer));
  }, []);

  const value = useMemo(() => ({ toasts, show, dismiss }), [toasts, show, dismiss]);
  return <ToastContext.Provider value={value}>{children}</ToastContext.Provider>;
}

export function useToasts(): ToastContextValue {
  const value = useContext(ToastContext);
  if (value === null) throw new Error("useToasts must be used inside a ToastProvider");
  return value;
}
