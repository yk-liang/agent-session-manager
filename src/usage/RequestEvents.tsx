import { useMemo, useState } from "react";
import { ChevronFirst, ChevronLast, ChevronLeft, ChevronRight, LoaderCircle } from "lucide-react";
import { useDelayedBusy } from "../useDelayedBusy";
import type { AgentInfo } from "../types";
import type { EventQuery, RequestFilters, RequestOutcome } from "./types";
import { formatCount } from "./format";
import { summaryQuery, type UsagePeriod } from "./range";
import { REQUEST_OUTCOMES, REQUEST_OUTCOME_IDS } from "./trend";
import { shortPath } from "../utils";
import UiIcon from "../components/UiIcon";
import { ModelIcon } from "../components/BrandIcon";
import Dropdown from "../components/Dropdown";
import { useRequestPage } from "./useRequestPage";
import RequestTable from "./RequestTable";
import RequestTimeline from "./RequestTimeline";

interface Props {
  agents: AgentInfo[];
  period: UsagePeriod;
  now: Date;
  agent: string;
  revision: string;
  selection: RequestSelection;
  onSelectionChange: (selection: RequestSelection) => void;
  syncing?: boolean;
}

export interface RequestSelection {
  model: string | null;
  project: string | null;
  status: RequestOutcome | null;
}

export const defaultRequestSelection: RequestSelection = { model: null, project: null, status: null };
const pageSizeOptions = [50, 100, 200].map(size => ({ id: String(size), name: `${size} 条/页` }));
const SEARCH_THRESHOLD = 8;
const EMPTY_FILTERS: RequestFilters = { models: {}, projects: {}, results: {} };

const allOption = "all";
const filterKey = (value: string | null) => value === null ? allOption : `value:${value}`;
const filterValue = (option: string) => option === allOption ? null : option.slice(6);

function filterOptions(
  counts: Record<string, number>,
  selected: string | null,
  allLabel: string,
  label: (id: string) => string = id => id,
  knownIds = Object.keys(counts),
) {
  const ids = new Set(knownIds);
  if (selected !== null) ids.add(selected);
  const entries = [...ids].sort((a, b) => (counts[b] ?? 0) - (counts[a] ?? 0) || label(a).localeCompare(label(b)));
  return [
    { id: allOption, name: allLabel, count: Object.values(counts).reduce((sum, count) => sum + count, 0) },
    ...entries.map(id => ({ id: filterKey(id), name: label(id), count: counts[id] ?? 0 })),
  ];
}

export default function RequestEvents({ agents, period, now, agent, revision, selection, onSelectionChange, syncing = false }: Props) {
  const { model, project, status } = selection;
  const [pageSize, setPageSize] = useState(50);
  const [copyError, setCopyError] = useState("");
  const { from, to } = summaryQuery(period, now);
  const query: EventQuery = {
    agent: agent === "all" ? undefined : agent,
    model: model ?? undefined,
    project: project ?? undefined,
    status: status ?? undefined,
    from, to, limit: pageSize,
  };
  const { filters, page, pageCount, pageInput, pageInputRef, setPageInput, table,
    loading, navigating, error, retryLoad, goToPage, expandedRequestId, setExpandedRequestId } = useRequestPage(query, revision);
  const showLoading = useDelayedBusy(navigating);
  const events = page?.events ?? [];
  const copyFeedback = useMemo(() => ({
    onCopied: () => setCopyError(""),
    onError: () => setCopyError("复制失败，请重试。"),
  }), []);
  const changePage = (target: number) => {
    setCopyError("");
    goToPage(target);
  };
  const { models, projects, results } = filters ?? EMPTY_FILTERS;
  const modelOptions = filterOptions(models, model, "全部模型")
    .map(option => ({ ...option, icon: option.id !== "all" ? <ModelIcon id={filterValue(option.id) ?? ""} /> : undefined }));
  const projectOptions = filterOptions(projects, project, "全部项目", id => shortPath(id) || "项目未记录");
  const resultOptions = filterOptions(results, status, "全部结果", id => REQUEST_OUTCOMES[id as RequestOutcome].label, REQUEST_OUTCOME_IDS);
  const filtered = model !== null || project !== null || status !== null;
  return <section className="usage-section request-events" aria-label="请求记录">
    <div className="request-heading">
      <h2>请求记录</h2>
      <div className="request-filters">
        <Dropdown label="模型" className="request-filter" fill value={filterKey(model)} options={modelOptions} countUnit="条请求记录"
          searchLabel={modelOptions.length > SEARCH_THRESHOLD ? "搜索模型" : undefined} onChange={model => onSelectionChange({ ...selection, model: filterValue(model) })} />
        <Dropdown label="项目" className="request-filter" fill value={filterKey(project)} options={projectOptions} countUnit="条请求记录"
          searchLabel={projectOptions.length > SEARCH_THRESHOLD ? "搜索项目" : undefined} onChange={project => onSelectionChange({ ...selection, project: filterValue(project) })} />
        <Dropdown label="请求结果" value={filterKey(status)} options={resultOptions} countUnit="条请求记录"
          onChange={status => onSelectionChange({ ...selection, status: filterValue(status) as RequestSelection["status"] })} />
        {filtered && <button className="request-clear" onClick={() => onSelectionChange(defaultRequestSelection)}>清除筛选</button>}
      </div>
    </div>
    <div className="request-table-wrap" ref={table} tabIndex={0} role="region" aria-label="请求记录表" aria-busy={loading}>
      {page && events.length > 0 && <RequestTable page={page} agents={agents} expandedRequestId={expandedRequestId}
        onExpand={setExpandedRequestId} copyFeedback={copyFeedback} />}
      {!events.length && !error && <div className="request-empty">{loading ? "正在读取请求记录…" : syncing ? "正在同步历史用量…" : filtered ? "未找到匹配的请求" : "所选范围暂无请求记录"}</div>}
    </div>
    {error && <div className="request-error" role="alert">无法读取请求记录：{error}<button disabled={navigating} onClick={retryLoad}>重试</button></div>}
    {copyError && <div className="request-notice" role="status">{copyError}</div>}
    <div className="request-footer">
      {page?.timeline && page.total > 0 && <RequestTimeline
        timeline={page.timeline}
        pageTimes={events.map(e => e.time).filter(t => t > 0)}
        pageSize={page.pageSize}
        page={page.page}
        now={Date.now()}
        onJump={changePage}
      />}
      <div className="request-footer-row">
        <span className="request-range" role="status">
          {showLoading && <UiIcon icon={LoaderCircle} className="usage-spin" />}
          {page && (!syncing || page.total > 0) && (page.total ? `第 ${formatCount((page.page - 1) * page.pageSize + 1)}–${formatCount((page.page - 1) * page.pageSize + events.length)} 条，共 ${formatCount(page.total)} 条` : "共 0 条")}
          {syncing && page && page.total > 0 && <small className="request-syncing">历史用量同步中</small>}
        </span>
        {page && <nav className="request-pagination" aria-label="请求记录分页">
          <Dropdown label="每页请求数" value={String(pageSize)} options={pageSizeOptions}
            disabled={navigating} onChange={id => setPageSize(Number(id))} />
          <div className="request-page-controls">
            <button aria-label="首页" title="首页" disabled={navigating || page.page <= 1} onClick={() => changePage(1)}><UiIcon icon={ChevronFirst} /></button>
            <button aria-label="上一页" title="上一页" disabled={navigating || page.page <= 1} onClick={() => changePage(page.page - 1)}><UiIcon icon={ChevronLeft} /></button>
            <span className="request-page-number">第 {formatCount(page.page)} / {formatCount(pageCount)} 页</span>
            <button aria-label="下一页" title="下一页" disabled={navigating || page.page >= pageCount} onClick={() => changePage(page.page + 1)}><UiIcon icon={ChevronRight} /></button>
            <button aria-label="末页" title="末页" disabled={navigating || page.page >= pageCount} onClick={() => changePage(pageCount)}><UiIcon icon={ChevronLast} /></button>
          </div>
          <form className="request-jump" noValidate onSubmit={e => { e.preventDefault(); changePage(pageInput.trim() ? Number(pageInput) : page.page); }}>
            <label>前往<input ref={pageInputRef} className="control-input" aria-label="请求页码" type="number" inputMode="numeric" min={1} max={pageCount} value={pageInput} disabled={navigating} onChange={e => setPageInput(e.target.value)} /></label>
            <button type="submit" disabled={navigating}>跳转</button>
          </form>
        </nav>}
      </div>
    </div>
  </section>;
}
