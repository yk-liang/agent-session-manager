import { useEffect, useState } from "react";
import { Pause, Play, Settings } from "lucide-react";
import * as api from "../api";
import type { CollectorStatus } from "../types";
import UiIcon from "./UiIcon";
import HelpTip from "./HelpTip";

/** 服务状态来自原生后台进程，不能用窗口是否打开推断正在采集。 */
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
  const approval = status.desired && status.authorization === "requiresApproval";
  const signingRequired = status.authorization === "signingRequired";
  const label = signingRequired ? "按需同步" : approval ? "等待系统授权" : status.running ? "后台保留已开启" : status.desired ? "后台保留未运行" : "按需同步";
  const change = async () => {
    setBusy(true); setError("");
    try {
      if (approval) await api.openCollectorSettings();
      else setStatus(await api.setCollectorEnabled(!status.desired));
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  return <span className="collector-control">
    <span className={`collector-dot${status.running ? " running" : ""}`} aria-hidden="true" />
    <span title={error || status.error || (status.running ? "关闭窗口或退出应用后继续采集，登录后自动启动" : label)}>{error ? "后台采集操作失败" : label}</span>
    <HelpTip label="同步与保留" align="end">
      {signingRequired ? <>使用应用时自动更新用量。<p>此版本不支持退出后的后台采集。</p></> : <>
        用量会自动更新。开启后台保留后，退出应用也会继续记录。
        <p>外部删除前尚未保存的用量无法找回。</p>
      </>}
    </HelpTip>
    {!signingRequired && <button disabled={busy} onClick={() => void change()} aria-label={approval ? "打开后台采集系统设置" : status.desired ? "暂停后台采集" : "开启后台采集"} title={approval ? "打开系统设置授权" : status.desired ? "暂停后台采集" : "开启后台采集"}>
      <UiIcon icon={approval ? Settings : status.desired ? Pause : Play} />
    </button>}
    {approval && <button disabled={busy} onClick={() => { setBusy(true); void api.setCollectorEnabled(false).then(setStatus).catch(cause => setError(String(cause))).finally(() => setBusy(false)); }}>暂停</button>}
  </span>;
}
