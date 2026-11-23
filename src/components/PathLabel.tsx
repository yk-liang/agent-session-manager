import { useLayoutEffect, useRef, useState } from "react";
import { fitPath } from "../utils";

let pathMeasure: CanvasRenderingContext2D | null = null;

interface Props {
  path: string;
  className?: string;
  bounds?: string;
  reservedWidth?: number;
  fontKey?: boolean | string;
}

export default function PathLabel({ path, className, bounds, reservedWidth = 0, fontKey }: Props) {
  const ref = useRef<HTMLSpanElement>(null);
  const [label, setLabel] = useState(path);

  useLayoutEffect(() => {
    const element = ref.current;
    const container = bounds ? element?.closest<HTMLElement>(bounds) : element;
    if (!element || !container) return;
    const measure = pathMeasure ??= document.createElement("canvas").getContext("2d");
    if (!measure) return;
    const update = () => {
      const rawWidth = bounds
        ? (() => {
            const style = getComputedStyle(container);
            return container.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
          })()
        : container.getBoundingClientRect().width;
      const width = Math.max(0, rawWidth - reservedWidth);
      const { fontFamily, fontWeight, fontSize } = getComputedStyle(element);
      measure.font = `${fontWeight} ${fontSize} ${fontFamily}`;
      setLabel(fitPath(path, text => measure.measureText(text).width <= width));
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(container);
    return () => observer.disconnect();
  }, [path, bounds, reservedWidth, fontKey]);

  return <span ref={ref} className={className} aria-label={path}>{label}</span>;
}
