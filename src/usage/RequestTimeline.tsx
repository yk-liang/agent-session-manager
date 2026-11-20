import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent } from "react";
import type { RequestTimelineData } from "./types";
import { formatCount, formatHourMinute, formatMonthDay, formatSpan } from "./format";
import { REQUEST_OUTCOMES, REQUEST_OUTCOME_IDS } from "./trend";
import { anchorOf, useTooltip } from "./UsageTooltip";

const TICK_MINUTES = [5, 10, 15, 30, 60, 120, 180, 360, 720, 1440, 2880, 10080, 20160, 43200, 131040, 525600];

/** 至多六个落在整点或零点的刻度；零点显示日期。 */
function axisTicks(start: number, end: number) {
  const minutes = TICK_MINUTES.find(m => (end - start) / 60000 / m <= 6) ?? 525600;
  const at = new Date(start);
  at.setHours(0, 0, 0, 0);
  const advance = () => (minutes < 1440 ? at.setMinutes(at.getMinutes() + minutes) : at.setDate(at.getDate() + minutes / 1440));
  while (at.getTime() < start) advance();
  const ticks: { time: number; label: string }[] = [];
  for (; at.getTime() < end && ticks.length < 12; advance()) {
    const time = at.getTime();
    ticks.push({ time, label: at.getHours() || at.getMinutes() ? formatHourMinute(time) : formatMonthDay(time) });
  }
  return ticks;
}

export default function RequestTimeline({ timeline, pageTimes, pageSize, page, now, onJump }: {
  timeline: RequestTimelineData;
  pageTimes: number[];
  pageSize: number;
  page: number;
  now: number;
  onJump: (page: number) => void;
}) {
  const root = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState<number | null>(null);
  const tip = useTooltip();
  const bucketCount = timeline.success.length;
  const activeBucket = active != null && active < bucketCount ? active : null;
  const hideTooltip = tip.hide;
  useEffect(() => {
    setActive(null);
    hideTooltip();
  }, [timeline, hideTooltip]);
  const counts = useMemo(
    () => timeline.success.map((count, i) => count + timeline.failed[i]),
    [timeline],
  );
  // 倒序分页按更晚时段的记录数计算页码。
  const newerRecordCounts = useMemo(() => {
    const result = new Array<number>(bucketCount).fill(0);
    for (let i = bucketCount - 2; i >= 0; i--) result[i] = result[i + 1] + counts[i + 1];
    return result;
  }, [counts, bucketCount]);
  const timed = bucketCount ? newerRecordCounts[0] + counts[0] : 0;
  const untimed = timeline.untimed.success + timeline.untimed.failed;
  const max = Math.max(1, ...counts);
  const { bounds, daysPerBucket } = timeline;
  const start = bounds[0];
  const end = bounds[bucketCount];
  const pageOf = (offset: number) => Math.floor(offset / pageSize) + 1;
  const bucketOf = (time: number) => {
    const next = bounds.findIndex(bound => bound > time);
    return next < 0 ? bucketCount - 1 : Math.max(0, next - 1);
  };
  const current = pageTimes.length ? [bucketOf(Math.min(...pageTimes)), bucketOf(Math.max(...pageTimes))] : null;
  const x = (ms: number) => {
    const i = bucketOf(ms);
    const fraction = (ms - bounds[i]) / (bounds[i + 1] - bounds[i]);
    return ((i + fraction) / bucketCount) * 100;
  };
  const openEnd = now < end && now >= start;

  const label = (i: number) => formatSpan(bounds[i], bounds[i + 1]);
  const describe = (i: number) =>
    counts[i] ? `${label(i)}，${formatCount(counts[i])} 条，失败 ${formatCount(timeline.failed[i])} 条，位于第 ${formatCount(pageOf(newerRecordCounts[i]))} 页` : `${label(i)}，没有记录`;

  const show = (i: number) => {
    setActive(i);
    const bar = root.current?.querySelectorAll(".request-timeline-bucket")[i];
    if (!bar) return;
    tip.show(
      {
        title: label(i),
        rows: counts[i]
          ? REQUEST_OUTCOME_IDS.filter(key => timeline[key][i] > 0).map(key => ({
              key, ...REQUEST_OUTCOMES[key], value: formatCount(timeline[key][i]),
            }))
          : [],
        total: { label: "合计", value: formatCount(counts[i]) },
        note: counts[i] ? `跳转到第 ${formatCount(pageOf(newerRecordCounts[i]))} 页` : "此时段暂无记录",
      },
      anchorOf(bar),
    );
  };
  const hide = () => {
    setActive(null);
    tip.hide();
  };
  const indexAt = (event: MouseEvent<HTMLDivElement>) => {
    const box = event.currentTarget.getBoundingClientRect();
    return Math.min(bucketCount - 1, Math.max(0, Math.floor(((event.clientX - box.left) / box.width) * bucketCount)));
  };
  const jump = (i: number) => counts[i] > 0 && onJump(pageOf(newerRecordCounts[i]));
  const onKey = (event: KeyboardEvent) => {
    const at = activeBucket ?? current?.[1] ?? bucketCount - 1;
    const next = ({ ArrowLeft: at - 1, ArrowRight: at + 1, Home: 0, End: bucketCount - 1 } as Record<string, number>)[event.key];
    if (next !== undefined) show(Math.min(bucketCount - 1, Math.max(0, next)));
    else if (event.key === "Enter" || event.key === " ") jump(at);
    else if (event.key === "Escape") hide();
    else return;
    event.preventDefault();
  };

  if (!bucketCount || timed === 0) {
    return untimed > 0 ? <div className="request-timeline-meta">{formatCount(untimed)} 条缺少时间</div> : null;
  }
  return (
    <div className="request-timeline" ref={root}>
      <div
        className={`request-timeline-plot${activeBucket != null && counts[activeBucket] ? " pickable" : ""}`}
        role="slider"
        tabIndex={0}
        aria-label="请求时间分布，方向键选择时段，Enter 跳到该时段"
        aria-valuemin={0}
        aria-valuemax={bucketCount - 1}
        aria-valuenow={activeBucket ?? current?.[1] ?? bucketCount - 1}
        aria-valuetext={describe(activeBucket ?? current?.[1] ?? bucketCount - 1)}
        aria-describedby={tip.open ? tip.id : undefined}
        onKeyDown={onKey}
        onBlur={hide}
        onPointerMove={event => {
          if (event.pointerType === "touch") return;
          const index = indexAt(event);
          if (index !== activeBucket || !tip.open) show(index);
        }}
        onPointerLeave={event => event.pointerType !== "touch" && hide()}
        onClick={event => jump(indexAt(event))}
      >
        {current && (
          <div
            className="request-timeline-band"
            style={{ left: `${(current[0] / bucketCount) * 100}%`, width: `${((current[1] - current[0] + 1) / bucketCount) * 100}%` }}
            aria-label={`第 ${formatCount(page)} 页所在时段`}
          />
        )}
        <div className="request-timeline-bars">
          {counts.map((count, i) => (
            <div
              key={i}
              className={`request-timeline-bucket${current && i >= current[0] && i <= current[1] ? " current" : ""}${activeBucket === i ? " active" : ""}`}
            >
              {count > 0 && REQUEST_OUTCOME_IDS.map(key => timeline[key][i] > 0 && (
                <i key={key} style={{ height: `max(2px, ${(timeline[key][i] / max) * 100}%)`, background: REQUEST_OUTCOMES[key].color }} />
              ))}
            </div>
          ))}
        </div>
      </div>
      {tip.tooltip}
      <div className="request-timeline-axis" aria-hidden="true">
        {axisTicks(start, end)
          .filter(({ time }) => x(time) > 3 && x(time) < (openEnd ? 88 : 97))
          .map(({ time, label: text }) => <span key={time} style={{ left: `${x(time)}%` }}>{text}</span>)}
        {openEnd && <span style={{ left: `clamp(1em, ${x(now)}%, calc(100% - 1em))` }}>现在</span>}
      </div>
      {(untimed > 0 || daysPerBucket != null) && <div className="request-timeline-meta">
        {untimed > 0 && (
          <button type="button" className="request-timeline-untimed btn-sm" onClick={() => onJump(pageOf(timed))}
            title="跳到缺少时间的记录">
            {formatCount(untimed)} 条缺少时间
          </button>
        )}
        {daysPerBucket != null && <span className="request-timeline-step">每段 {formatCount(daysPerBucket)} 天</span>}
      </div>}
    </div>
  );
}
