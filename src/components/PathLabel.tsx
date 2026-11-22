import { useLayoutEffect, useRef, useState } from "react";

let pathMeasure: CanvasRenderingContext2D | null = null;

export function fitPath(path: string, fits: (label: string) => boolean) {
  if (fits(path)) return path;

  const parts = path.split("/");
  const trailingSlash = path.endsWith("/");
  if (trailingSlash) parts.pop();
  const leaf = parts.pop();
  if (!parts.length || !leaf) return path;

  const suffix = trailingSlash ? `${leaf}/` : leaf;
  const compact = [...parts];
  for (const [index, part] of parts.entries()) {
    if (!part || part === "~" || part === "." || part === "..") continue;
    compact[index] = Array.from(part).slice(0, part.startsWith(".") ? 2 : 1).join("");
    const label = [...compact, suffix].join("/");
    if (fits(label)) return label;
  }
  for (let start = 1; start <= parts.length; start++) {
    const label = ["…", ...parts.slice(start), suffix].join("/");
    if (fits(label)) return label;
  }
  return suffix;
}

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
