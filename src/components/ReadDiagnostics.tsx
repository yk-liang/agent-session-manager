import { useState } from "react";
import { ChevronRight } from "lucide-react";
import CopyButton from "./CopyButton";
import UiIcon from "./UiIcon";

interface Props {
  title: string;
  errors: string[];
  retry?: () => void;
  busy?: boolean;
  className?: string;
}

export default function ReadDiagnostics({ title, errors, retry, busy = false, className = "" }: Props) {
  const [copyError, setCopyError] = useState("");
  const details = errors.join("\n\n---\n\n");
  const diagnosticFields = [["阶段", "stage"], ["类型", "error_kind"], ["系统错误码", "os_code"]]
    .flatMap(([label, field]) => {
      const value = errors[0]?.match(new RegExp(`^${field}: (.+)$`, "m"))?.[1];
      return value ? [{ label, value }] : [];
    });
  return <section className={`read-diagnostics ${className}`} role="alert" aria-label={title}>
    <strong>{title}</strong>
    {errors.length === 1 && <p className="read-error-summary">{errors[0].split("\n")[0]}</p>}
    {errors.length > 1 && diagnosticFields.length > 0 && <p className="read-error-summary">第一条诊断</p>}
    {diagnosticFields.length > 0 && <dl className="read-error-fields">
      {diagnosticFields.map(({ label, value }) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}
    </dl>}
    <div className="read-error-actions">
      {retry && <button disabled={busy} onClick={retry}>{busy ? "重试中…" : "重试"}</button>}
      <CopyButton value={details} label="复制诊断详情" className="copy-diagnostics" onError={e => setCopyError(`复制失败：${String(e)}`)} onCopied={() => setCopyError("")}>复制诊断详情</CopyButton>
    </div>
    <details><summary><UiIcon icon={ChevronRight} className="details-caret" />诊断详情{errors.length > 1 ? `（${errors.length} 条）` : ""}</summary>
      <p className="diagnostic-privacy">诊断可能包含本机路径，请在分享前检查。</p>
      <pre>{details}</pre>
    </details>
    {copyError && <p>{copyError}</p>}
  </section>;
}
