import { useSyncExternalStore } from 'react';

/** A value with subscribers. Updates are immutable: `set` takes the old state and returns the new one. */
export function createStore<T>(initial: T) {
  let state = initial;
  const listeners = new Set<() => void>();
  return {
    get: () => state,
    set(update: (s: T) => T) {
      const next = update(state);
      if (next === state) return;
      state = next;
      listeners.forEach((l) => l());
    },
    subscribe(l: () => void) {
      listeners.add(l);
      return () => listeners.delete(l);
    },
  };
}

export type Store<T> = ReturnType<typeof createStore<T>>;

/** Reads part of a store. The selector must return a value that is stable while the state is (a field, not a new object). */
export function useStore<T, R>(store: Store<T>, select: (s: T) => R): R {
  return useSyncExternalStore(store.subscribe, () => select(store.get()));
}
