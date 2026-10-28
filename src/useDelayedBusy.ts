import { useEffect, useRef, useState } from "react";

/** 只延迟视觉提示，不延迟数据展示或操作互斥；短任务无需闪一下加载图标。 */
export function useDelayedBusy(busy: boolean, delayMs = 300, minimumVisibleMs = 300) {
  const [visible, setVisible] = useState(false);
  const shownAt = useRef<number | null>(null);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    if (busy) {
      if (shownAt.current === null) timer = setTimeout(() => {
        shownAt.current = performance.now();
        setVisible(true);
      }, delayMs);
    } else {
      const remaining = shownAt.current === null ? 0 : Math.max(0, minimumVisibleMs - (performance.now() - shownAt.current));
      timer = setTimeout(() => { shownAt.current = null; setVisible(false); }, remaining);
    }
    return () => clearTimeout(timer);
  }, [busy, delayMs, minimumVisibleMs]);
  return visible;
}
