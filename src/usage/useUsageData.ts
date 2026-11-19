import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";
import { dateKey } from "./range";
import type { UsageSummary, UsageSummaryQuery, UsageSyncState } from "./types";

const summaries = new Map<string, UsageSummary>();

export function useUsageDate() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>;
    const update = () => {
      const current = new Date();
      setNow(current);
      const midnight = new Date(
        current.getFullYear(),
        current.getMonth(),
        current.getDate() + 1,
      );
      timer = setTimeout(update, midnight.getTime() - current.getTime() + 50);
    };
    update();
    const focus = () => {
      clearTimeout(timer);
      update();
    };
    window.addEventListener("focus", focus);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("focus", focus);
    };
  }, []);

  return now;
}

export function useUsageSync() {
  const [syncStatus, setSyncStatus] = useState<UsageSyncState | null>(null);
  const [connectionFailed, setConnectionFailed] = useState(false);
  const [pricingRevision, setPricingRevision] = useState(0);
  const onPricingChanged = useCallback((revision: number) => {
    if (Number.isFinite(revision)) setPricingRevision(current => Math.max(current, revision));
  }, []);

  useEffect(() => {
    let active = true;
    let poll: ReturnType<typeof setTimeout> | undefined;
    const update = (status: UsageSyncState) => {
      if (!active) return;
      setSyncStatus(current => !current || status.sequence >= current.sequence ? status : current);
      setConnectionFailed(false);
    };

    const readStatus = async (repeat: boolean) => {
      if (!active) return;
      try {
        update(await api.usageSyncStatus());
      } catch {
        if (active) setConnectionFailed(true);
      }
      if (active && repeat) poll = setTimeout(() => void readStatus(true), 5000);
    };

    const status = listen<UsageSyncState>("usage-sync-status", ({ payload }) => update(payload));
    const pricing = listen<number>("usage-pricing-updated", ({ payload }) => {
      if (active) onPricingChanged(payload);
    }).catch(() => null);
    void status.then(() => readStatus(false), () => readStatus(true));

    return () => {
      active = false;
      clearTimeout(poll);
      void status.then(unlisten => unlisten()).catch(() => {});
      void pricing.then(unlisten => unlisten?.()).catch(() => {});
    };
  }, [onPricingChanged]);

  const checkUsage = () => {
    void api.checkUsageNow().catch(() => setConnectionFailed(true));
  };
  return {
    syncStatus, syncUnavailable: connectionFailed || syncStatus?.unavailable === true,
    pricingRevision, onPricingChanged, checkUsage,
  };
}

export function useUsageSummary(query: UsageSummaryQuery, eventsRevision: number, pricingRevision: number) {
  const [summaryRequest, setSummaryRequest] = useState<{ key: string; loading: boolean; error?: string }>();
  const [queryRetry, setQueryRetry] = useState(0);
  const queryKey = JSON.stringify(query);
  const summary = summaries.get(queryKey) ?? null;
  const queryLoading = summaryRequest?.key !== queryKey || summaryRequest.loading;
  const queryError = summaryRequest?.key === queryKey ? summaryRequest.error : undefined;

  useEffect(() => {
    let disposed = false;
    setSummaryRequest({ key: queryKey, loading: true });
    const generation = api.nextSummaryGeneration();
    void api
      .readUsageSummary(JSON.parse(queryKey) as UsageSummaryQuery, generation)
      .then(value => {
        if (disposed) return;
        const normalized = {
          ...value,
          rows: value.rows.map(row => ({
            ...row,
            date: row.bucketStart == null ? row.date : dateKey(new Date(row.bucketStart)),
          })),
        };
        summaries.set(queryKey, normalized);
        while (summaries.size > 8) summaries.delete(summaries.keys().next().value!);
        setSummaryRequest({ key: queryKey, loading: false });
      })
      .catch(error => {
        if (!disposed) {
          setSummaryRequest({ key: queryKey, loading: false, error: String(error) });
        }
      });
    return () => {
      disposed = true;
      void api.cancelUsageSummary().catch(() => {});
    };
  }, [queryKey, eventsRevision, pricingRevision, queryRetry]);

  return { summary, queryLoading, queryError, retrySummary: () => setQueryRetry(value => value + 1) };
}
