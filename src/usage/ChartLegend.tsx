export interface LegendItem {
  key: string;
  label: string;
  color: string;
  /** 补充值，例如合计或占比。 */
  detail?: string;
}

/**
 * 胶囊图例：圆点 + 文字。传入 hidden/onToggle 时每项是可切换按钮，至少保留一项可见；
 * 否则只是说明。onHighlight 用于悬停时突出对应序列。
 */
export default function ChartLegend({ items, label, hidden, onToggle, onHighlight }: {
  items: LegendItem[];
  label: string;
  hidden?: ReadonlySet<string>;
  onToggle?: (key: string) => void;
  onHighlight?: (key: string | null) => void;
}) {
  if (!items.length) return null;
  const visible = items.filter(item => !hidden?.has(item.key)).length;
  return (
    <div className="chart-legend" role={onToggle ? "group" : "list"} aria-label={label}>
      {items.map(item => {
        const off = !!hidden?.has(item.key);
        const content = (
          <>
            <i style={{ background: item.color }} aria-hidden="true" />
            <span>{item.label}</span>
            {item.detail && <small>{item.detail}</small>}
          </>
        );
        return onToggle ? (
          <button
            key={item.key}
            type="button"
            className={`chart-legend-item${off ? " off" : ""}`}
            aria-pressed={!off}
            disabled={!off && visible <= 1}
            onClick={() => onToggle(item.key)}
            onPointerEnter={() => onHighlight?.(off ? null : item.key)}
            onPointerLeave={() => onHighlight?.(null)}
            onFocus={() => onHighlight?.(off ? null : item.key)}
            onBlur={() => onHighlight?.(null)}
          >
            {content}
          </button>
        ) : (
          <span key={item.key} role="listitem" className="chart-legend-item">
            {content}
          </span>
        );
      })}
    </div>
  );
}
