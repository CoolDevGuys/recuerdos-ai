/**
 * Where the API key lives between reloads: `sessionStorage`, so it is gone
 * when the tab closes and never shared with other tabs or written to disk
 * by `localStorage` (plan §6). Storage can be unavailable (private modes,
 * blocked site data), so every access is guarded and the app simply asks
 * for the key again.
 */
export interface KeyStore {
  read(): string | null;
  write(key: string): void;
  clear(): void;
}

const STORAGE_KEY = "recuerdos.apiKey";

export const sessionKeyStore: KeyStore = {
  read() {
    try {
      return window.sessionStorage.getItem(STORAGE_KEY);
    } catch {
      return null;
    }
  },
  write(key) {
    try {
      window.sessionStorage.setItem(STORAGE_KEY, key);
    } catch {
      // Not persisted: the user signs in again after a reload.
    }
  },
  clear() {
    try {
      window.sessionStorage.removeItem(STORAGE_KEY);
    } catch {
      // Nothing to clear.
    }
  },
};

/** A store that remembers in memory only, for tests. */
export function memoryKeyStore(initial: string | null = null): KeyStore {
  let value = initial;
  return {
    read: () => value,
    write: (key) => {
      value = key;
    },
    clear: () => {
      value = null;
    },
  };
}
