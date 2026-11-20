import { useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { formatTokens, formatCount, formatUsd } from "./format";
import { costGapNotes, type TrendMetric } from "./metrics";
import { TREND_METRIC_NAMES, type StackedSeries, type UsagePoint } from "./trend";
import ChartLegend from "./ChartLegend";
import { useTooltip, type TooltipContent } from "./UsageTooltip";

const LEFT = 64;
const RIGHT = 24;
const TOP = 14;
const BASE = 172;
const HEIGHT = 200;

const formatValue = (metric: TrendMetric, value: number) => (metric === "cost" ? formatUsd(value) : metric === "tokens" ? formatTokens(value) : formatCount(value));

function barPath(x: number, y: number, w: number, h: number, r: number) {
  const radius = Math.min(r, w / 2, h);
  return `M${x},${y + h}V${y + radius}Q${x},${y} ${x + radius},${y}H${x + w - radius}Q${x + w},${y} ${x + w},${y + radius}V${y + h}Z`;
}

/** 将坐标轴上限取整到 1/2/2.5/5 × 10^n，刻度更易读。 */
function niceMax(value: number, metric: TrendMetric) {
  const floor = metric === "cost" ? 0.0001 : 1;
  if (!(value > floor)) return floor;
  const magnitude = 10 ** Math.floor(Math.log10(value));
  const step = [1, 2, 2.5, 5, 10].find(f => f * magnitude >= value) ?? 10;
  return step * magnitude;
}

export default function UsageTrendChart({ points, series, metric, hourly }: {
  points: UsagePoint[];
  series: StackedSeries[];
  metric: TrendMetric;
  hourly: boolean;
}) {
  const container = useRef<HTMLDivElement>(null);
  const plot = useRef<SVGSVGElement>(null);
  const [width, setWidth] = useState(0);
  const [hiddenKeys, setHidden] = useState<Set<string>>(() => new Set());
  const [highlight, setHighlight] = useState<string | null>(null);
  const [active, setActive] = useState<number | null>(null);
  const tip = useTooltip();

  useLayoutEffect(() => {
    const element = container.current;
    if (!element) return;
    const measure = () => setWidth(element.clientWidth);
    const observer = new ResizeObserver(measure);
    measure();
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const hidden = useMemo(() => {
    const keys = series.filter(item => hiddenKeys.has(item.key)).map(item => item.key);
    return new Set(keys.length === series.length ? [] : keys);
  }, [series, hiddenKeys]);
  const visible = useMemo(() => series.filter(item => !hidden.has(item.key)), [series, hidden]);
  const totals = useMemo(() => points.map((_, i) => visible.reduce((sum, item) => sum + (item.values[i] ?? 0), 0)), [points, visible]);
  const max = niceMax(Math.max(0, ...totals), metric);
  const plotWidth = Math.max(0, width - LEFT - RIGHT);
  const step = plotWidth / Math.max(1, points.length);
  const barWidth = Math.max(1, Math.min(40, step * 0.7));
  const y = (value: number) => BASE - (value / max) * (BASE - TOP);
  const ticks = [...new Set((hourly ? [0, 6, 12, 18, points.length - 1] : [0, Math.floor((points.length - 1) / 2), points.length - 1])
    .filter(i => i >= 0 && i < points.length))];

  const content = (index: number): TooltipContent => {
    const point = points[index];
    const rows = visible
      .map(item => ({ key: item.key, label: item.label, color: item.color, value: item.values[index] ?? 0 }))
      .filter(row => row.value > 0)
      .reverse()
      .map(row => ({ ...row, value: formatValue(metric, row.value) }));
    const note = metric === "cost"
      ? costGapNotes(point).join("；")
      : metric === "requests" && point.requestCountIncomplete
        ? "仅含可确认的请求次数"
        : undefined;
    return {
      title: point.title,
      rows,
      total: { label: hidden.size ? "显示项合计" : "合计", value: metric === "cost" && point.costUsd == null && point.recordCount > 0 ? "—" : formatValue(metric, totals[index]) },
      note,
    };
  };
  const showAt = (index: number) => {
    const svg = plot.current;
    if (!svg) return;
    const rect = svg.getBoundingClientRect();
    const x = rect.left + LEFT + (index + 0.5) * step;
    setActive(index);
    tip.show(content(index), { x, top: rect.top + y(totals[index]), bottom: rect.top + BASE });
  };
  const indexAt = (event: PointerEvent) => {
    const rect = plot.current?.getBoundingClientRect();
    if (!rect || !points.length) return null;
    const index = Math.floor((event.clientX - rect.left - LEFT) / step);
    return index >= 0 && index < points.length ? index : null;
  };
  const onPointer = (event: PointerEvent) => {
    const index = indexAt(event);
    if (index == null) leave();
    else if (index !== active || !tip.open) showAt(index);
  };
  const leave = () => {
    setActive(null);
    tip.hide();
  };
  const onKey = (event: KeyboardEvent) => {
    const current = active ?? points.length - 1;
    const next = ({ ArrowLeft: current - 1, ArrowRight: current + 1, Home: 0, End: points.length - 1 } as Record<string, number>)[event.key];
    if (event.key === "Escape") leave();
    else if (next === undefined) return;
    else showAt(Math.min(points.length - 1, Math.max(0, next)));
    event.preventDefault();
  };
  const toggle = (key: string) =>
    setHidden(() => {
      const next = new Set(hidden);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  if (!series.length) {
    return <div className="usage-chart-empty">{metric === "cost" ? "所选范围暂无可估算的费用" : "所选范围暂无记录"}</div>;
  }
  const metricName = TREND_METRIC_NAMES[metric];
  const activeText = active != null && points[active]
    ? `${points[active].title}，${metricName}${formatValue(metric, totals[active])}`
    : undefined;
  return (
    <div className="usage-chart" ref={container}>
      <div
        className="usage-chart-plot"
        role="slider"
        tabIndex={0}
        aria-label={`${hourly ? "每小时" : "每日"}${metricName}趋势，左右方向键逐个时段查看`}
        aria-valuemin={0}
        aria-valuemax={Math.max(0, points.length - 1)}
        aria-valuenow={active ?? points.length - 1}
        aria-valuetext={activeText}
        aria-describedby={tip.open ? tip.id : undefined}
        onKeyDown={onKey}
        onBlur={leave}
        onPointerMove={event => event.pointerType !== "touch" && onPointer(event)}
        onPointerDown={event => event.pointerType === "touch" && onPointer(event)}
        onPointerLeave={event => event.pointerType !== "touch" && leave()}
      >
        <svg ref={plot} height={HEIGHT} aria-hidden="true">
          {[0, 0.5, 1].map(ratio => (
            <g key={ratio}>
              <line x1={LEFT} x2={LEFT + plotWidth} y1={y(max * ratio)} y2={y(max * ratio)} className="usage-grid-line" />
              <text x={LEFT - 9} y={y(max * ratio) + 4} textAnchor="end">{formatValue(metric, max * ratio)}</text>
            </g>
          ))}
          {points.map((point, index) => {
            const x = LEFT + index * step + (step - barWidth) / 2;
            let base = 0;
            const segments = visible.map(item => {
              const value = item.values[index] ?? 0;
              const bottom = base;
              base += value;
              return { item, value, bottom, top: base };
            }).filter(s => s.value > 0);
            const last = segments.length - 1;
            return (
              <g key={point.key} className={`usage-bucket${active === index ? " active" : ""}`} data-bucket={point.key} data-total={point.totalTokens}>
                {active === index && <rect className="usage-bucket-cursor" x={LEFT + index * step} y={TOP} width={step} height={BASE - TOP} />}
                {segments.map(({ item, bottom, top }, i) => {
                  // 段间留 1px 面板色间隙；过矮的段保持 1px 可见。
                  const yTop = y(top);
                  const height = Math.max(1, y(bottom) - yTop - (i > 0 ? 1 : 0));
                  return (
                    <path
                      key={item.key}
                      d={i === last ? barPath(x, yTop, barWidth, height, 3) : `M${x},${yTop}h${barWidth}v${height}h${-barWidth}Z`}
                      fill={item.color}
                      className={highlight && highlight !== item.key ? "dimmed" : undefined}
                    />
                  );
                })}
              </g>
            );
          })}
          {ticks.map(i => (
            <text key={i} x={LEFT + (i + 0.5) * step} y={HEIGHT - 5} textAnchor="middle">{points[i].label}</text>
          ))}
        </svg>
      </div>
      {tip.tooltip}
      <ChartLegend
        label={`${metricName}序列`}
        items={series.map(item => ({ key: item.key, label: item.label, color: item.color }))}
        hidden={series.length > 1 ? hidden : undefined}
        onToggle={series.length > 1 ? toggle : undefined}
        onHighlight={setHighlight}
      />
    </div>
  );
}
