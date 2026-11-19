import type { Display } from "./format";

export default function UsageNumber({ value, className, showTitle = true }: { value: Display; className?: string; showTitle?: boolean }) {
  const numberText = value.prefix ? value.text.slice(value.prefix.length).trimStart() : value.text;
  return (
    <span className={`usage-num${value.missing ? " missing" : ""}${className ? ` ${className}` : ""}`} title={showTitle ? value.title : undefined}>
      {value.missing ? (
        <span aria-label={value.title}>—</span>
      ) : (
        <>
          {value.prefix && <span className="usage-num-prefix">{value.prefix} </span>}
          {numberText}
        </>
      )}
    </span>
  );
}
