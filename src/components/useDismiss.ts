import { useEffect, useEffectEvent, type RefObject } from "react";

interface Layer {
  roots: readonly RefObject<HTMLElement | null>[];
  dismiss: () => void;
}

const layers = new Set<Layer>();

function dismissOnEscape(event: KeyboardEvent) {
  if (event.key !== "Escape" || event.defaultPrevented || event.isComposing) return;
  const target = event.target instanceof Node ? event.target : null;
  const modal = target instanceof Element ? target.closest("[aria-modal=true]") : null;
  const eligible = [...layers].reverse().filter(layer => !modal || layer.roots.some(root => modal.contains(root.current)));
  const layer = eligible.find(layer => layer.roots.some(root => root.current?.contains(target))) ?? eligible[0];
  if (!layer) return;
  event.preventDefault();
  layer.dismiss();
}

export default function useDismiss(
  open: boolean,
  roots: readonly RefObject<HTMLElement | null>[],
  onDismiss: (reason: "outside" | "escape") => void,
  { focusOutside = false, windowBlur = false }: {
    focusOutside?: boolean;
    windowBlur?: boolean;
  } = {},
) {
  const outside = useEffectEvent((event: Event) => {
    if (!roots.some(root => root.current?.contains(event.target as Node))) onDismiss("outside");
  });
  const escape = useEffectEvent(() => onDismiss("escape"));
  const blur = useEffectEvent(() => onDismiss("outside"));

  useEffect(() => {
    if (!open) return;
    const layer = { roots, dismiss: escape };
    if (!layers.size) document.addEventListener("keydown", dismissOnEscape);
    layers.add(layer);
    document.addEventListener("pointerdown", outside);
    if (focusOutside) document.addEventListener("focusin", outside);
    if (windowBlur) window.addEventListener("blur", blur);
    return () => {
      document.removeEventListener("pointerdown", outside);
      layers.delete(layer);
      if (!layers.size) document.removeEventListener("keydown", dismissOnEscape);
      if (focusOutside) document.removeEventListener("focusin", outside);
      if (windowBlur) window.removeEventListener("blur", blur);
    };
  }, [open, focusOutside, windowBlur]);
}
