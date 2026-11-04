import { useState } from "react";
import CopyButton from "./CopyButton";

interface Props {
  title: string;
  errors: string[];
  description?: string;
  retry?: () => void;
  retryLabel?: string;
  busy?: boolean;
  className?: string;
}

/** 完整原始诊断留在可复制区域；正文只显示摘要，长路径不会挤占操作入口。 */
export default function ReadDiagnostics({ title, errors, description, retry, retryLabel = "重试读取", busy = false, className = "" }: Props) {
  const [copyError, setCopyError] = useState("");
  const details = errors.join("\n\n---\n\n");
  const fields = [["阶段", "stage"], ["类型", "error_kind"], ["系统码", "os_code"], ["数据库码", "sqlite_code"]]
    .flatMap(([label, field]) => {
      const value = errors[0]?.match(new RegExp(`^${field}: (.+)$`, "m"))?.[1];
      return value ? [`${label}：${value}`] : [];
    });
  return <section className={`read-diagnostics ${className}`} role="alert" aria-label={title}>
    <strong>{title}</strong>
    {description && <p>{description}</p>}
    {errors.length === 1 && <p className="read-error-summary">{errors[0].split("\n")[0]}</p>}
    {fields.length > 0 && <p className="read-error-fields">{errors.length > 1 && "首条诊断 · "}{fields.join(" · ")}</p>}
    <div className="read-error-actions">
      {retry && <button disabled={busy} onClick={retry}>{busy ? "正在重试…" : retryLabel}</button>}
      <CopyButton value={details} label="复制诊断详情" className="copy-diagnostics" onError={e => setCopyError(`复制失败：${String(e)}`)} onCopied={() => setCopyError("")}>复制诊断详情</CopyButton>
    </div>
    <details><summary>诊断详情{errors.length > 1 ? `（${errors.length} 条）` : ""}</summary>
      <p className="diagnostic-privacy">包含本机路径和进程信息，不包含会话正文。</p>
      <pre>{details}</pre>
    </details>
    {copyError && <p>{copyError}</p>}
  </section>;
}
