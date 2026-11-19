import { useEffect, useEffectEvent, useLayoutEffect, useRef, useState } from "react";
import * as api from "../api";
import type { EventPage, EventQuery, RequestFilters } from "./types";

interface ViewAnchor { id: string; isDetailRow: boolean; offset: number; scrollTop: number }

function readViewAnchor(table: HTMLDivElement | null): ViewAnchor | null {
  if (!table) return null;
  const top = table.getBoundingClientRect().top + (table.querySelector("thead")?.getBoundingClientRect().height ?? 0);
  const row = [...table.querySelectorAll<HTMLTableRowElement>("tr[data-event-id]")].find(row => row.getBoundingClientRect().bottom > top);
  return row ? { id: row.dataset.eventId!, isDetailRow: row.classList.contains("request-detail-row"), offset: row.getBoundingClientRect().top - top, scrollTop: table.scrollTop } : null;
}

export function useRequestPage(query: EventQuery, revision: string) {
  const [filters, setFilters] = useState<RequestFilters | null>(null);
  const filtersKey = useRef("");
  const [page, setPage] = useState<EventPage | null>(null);
  const [pageInput, setPageInput] = useState("1");
  const [queuedSync, setQueuedSync] = useState(0);
  const [phase, setPhase] = useState<"navigate" | "refresh" | null>(null);
  const navigating = phase === "navigate";
  const loading = phase !== null;
  const [error, setError] = useState("");
  const [expandedRequestId, setExpandedRequestId] = useState<string | null>(null);
  const requestSequence = useRef(0);
  const currentPage = useRef<EventPage | null>(null);
  const busy = useRef(false);
  const latestRevision = useRef(revision);
  latestRevision.current = revision;
  const table = useRef<HTMLDivElement>(null);
  const pageInputRef = useRef<HTMLInputElement>(null);
  const restoreView = useRef<ViewAnchor | "top" | null>(null);
  const retry = useRef({ page: 1, refresh: false });
  const queryKey = JSON.stringify(query);
  const loadPage = async (pageNumber: number, refresh = false) => {
    const requestId = ++requestSequence.current;
    const startedRevision = latestRevision.current;
    const readingHistory = currentPage.current?.page !== 1 || (table.current?.scrollTop ?? 0) > 0 || expandedRequestId !== null;
    const anchor = refresh && readingHistory ? readViewAnchor(table.current) : null;
    const generation = api.nextEventGeneration();
    retry.current = { page: pageNumber, refresh };
    busy.current = true;
    setPhase(refresh ? "refresh" : "navigate");
    setError("");
    try {
      const key = JSON.stringify({ ...query, limit: undefined, revision: startedRevision });
      const result = await api.readRequestEvents({
        ...query, page: pageNumber, includeFilters: filtersKey.current !== key, ...(anchor && { anchor: anchor.id }),
      }, generation);
      if (requestSequence.current !== requestId) return;
      if (!result || !Array.isArray(result.events)) throw new Error("请求记录格式无效");
      // 用户已滚动到新位置时，跳过这次刷新。
      if (anchor && table.current && Math.abs(table.current.scrollTop - anchor.scrollTop) > 1) return;
      restoreView.current = refresh ? anchor : "top";
      if (result.filters) {
        filtersKey.current = key;
        setFilters(result.filters);
      }
      currentPage.current = result;
      setPage(result);
      setExpandedRequestId(current => refresh && result.events.some(event => event.id === current) ? current : null);
      if (!refresh || document.activeElement !== pageInputRef.current) setPageInput(String(result.page));
    } catch (e) {
      if (requestSequence.current === requestId) setError(String(e));
    } finally {
      if (requestSequence.current === requestId) {
        busy.current = false;
        setPhase(null);
        // 翻页结束后补查这期间同步的新记录。
        if (latestRevision.current !== startedRevision) setQueuedSync(value => value + 1);
      }
    }
  };
  const navigate = useEffectEvent(() => void loadPage(1));
  const update = useEffectEvent(() => {
    if (!busy.current) void loadPage(currentPage.current?.page ?? 1, true);
  });
  useEffect(() => {
    currentPage.current = null;
    restoreView.current = null;
    setPage(null);
    setExpandedRequestId(null);
    setPageInput("1");
    navigate();
    return () => { ++requestSequence.current; busy.current = false; void api.cancelRequestEvents().catch(() => {}); };
  }, [queryKey]);
  useEffect(() => update(), [revision, queuedSync]);
  useLayoutEffect(() => {
    const view = restoreView.current, element = table.current;
    restoreView.current = null;
    if (!view || !element) return;
    if (view === "top") { element.scrollTop = 0; return; }
    const row = [...element.querySelectorAll<HTMLTableRowElement>("tr[data-event-id]")].find(row => row.dataset.eventId === view.id && row.classList.contains("request-detail-row") === view.isDetailRow);
    if (row) {
      const top = element.getBoundingClientRect().top + (element.querySelector("thead")?.getBoundingClientRect().height ?? 0);
      element.scrollTop += row.getBoundingClientRect().top - top - view.offset;
    }
  }, [page]);
  const pageCount = page ? Math.max(1, Math.ceil(page.total / page.pageSize)) : 1;
  const goToPage = (number: number) => {
    const target = Math.max(1, Math.min(pageCount, Number.isFinite(number) ? Math.trunc(number) : page?.page ?? 1));
    setPageInput(String(target));
    if (page && target !== page.page && !navigating) void loadPage(target);
  };
  const retryLoad = () => void loadPage(retry.current.page, retry.current.refresh);

  return {
    filters, page, pageCount, pageInput, pageInputRef, setPageInput, table,
    loading, navigating, error, retryLoad, goToPage, expandedRequestId, setExpandedRequestId,
  };
}
