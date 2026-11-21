import { useCallback, useSyncExternalStore } from "react";
import { createUiPreferenceStore, type PreferenceKey, type PreferenceUpdate } from "./uiPreferences";

const store = createUiPreferenceStore();

export function useUiPreference<K extends PreferenceKey>(key: K) {
  const value = useSyncExternalStore(store.subscribe, useCallback(() => store.get(key), [key]));
  const update = useCallback((next: PreferenceUpdate<K>) => store.set(key, next), [key]);
  return [value, update] as const;
}
