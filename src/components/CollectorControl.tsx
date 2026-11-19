import { useEffect, useState } from "react";
import { Pause, Play, Settings } from "lucide-react";
import * as api from "../api";
import type { CollectorStatus } from "../types";
import UiIcon from "./UiIcon";
import HelpTip from "./HelpTip";

export default function CollectorControl() {
  const [status, setStatus] = useState<CollectorStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let disposed = false;
    const check = () => void api.collectorStatus().then(value => { if (!disposed) setStatus(value); }).catch(() => {});
    check();
    const timer = setInterval(check, 10000);
    window.addEventListener("focus", check);
    return () => { disposed = true; clearInterval(timer); window.removeEventListener("focus", check); };
  }, []);
  if (!status || status.authorization === "unsupported" || status.authorization === "notBundled") return null;
  const needsApproval = status.desired && status.authorization === "requiresApproval";
  const signingRequired = status.authorization === "signingRequired";
  const statusLabel = signingRequired ? "按需同步" : needsApproval ? "等待系统授权" : status.running ? "后台采集已开启" : status.desired ? "后台采集未运行" : "按需同步";
  const actionLabel = needsApproval ? "打开系统设置" : status.desired ? "暂停后台采集" : "开启后台采集";
  const changeCollectorStatus = async () => {
    setBusy(true); setError("");
    try {
      if (needsApproval) await api.openCollectorSettings();
      else setStatus(await api.setCollectorEnabled(!status.desired));
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  return <span className="collector-control">
    <span className={`collector-dot${status.running ? " running" : ""}`} aria-hidden="true" />
    <span title={error || status.error || (status.running ? "登录后自动启动，退出应用后继续采集" : statusLabel)}>{error ? "后台采集操作失败" : statusLabel}</span>
    <HelpTip label="用量同步" align="end">
      {signingRequired ? "此版本仅在应用运行时同步用量。" : <>
        开启后台采集后，退出应用仍会记录用量。
        <p>其他应用删除日志前，未同步的用量可能丢失。</p>
      </>}
    </HelpTip>
    {!signingRequired && <button disabled={busy} onClick={() => void changeCollectorStatus()} aria-label={actionLabel} title={actionLabel}>
      <UiIcon icon={needsApproval ? Settings : status.desired ? Pause : Play} />
    </button>}
    {needsApproval && <button disabled={busy} onClick={() => { setBusy(true); void api.setCollectorEnabled(false).then(setStatus).catch(cause => setError(String(cause))).finally(() => setBusy(false)); }}>暂停</button>}
  </span>;
}
