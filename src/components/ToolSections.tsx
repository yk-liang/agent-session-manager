import { useContext, useEffect, useId, useState, type ReactNode } from "react";
import { itemStatus, type ToolItem } from "../activity";
import { readToolPayload } from "../api";
import type { ContentBlock, ToolOp } from "../types";
import CopyButton from "./CopyButton";
import { DetailState } from "./DetailState";
import Notice from "./Notice";
import Segmented from "./Segmented";
import { resultView, scriptView, type View } from "./ToolContent";

type Kind = "input" | "output" | "error";
type Views = (View | false | undefined)[];
type SectionView = View & { id?: string };

function sectionViewId(view: SectionView) {
  return view.id ?? view.label;
}

function Section({ kind, title, note, views, actions, onSelect }: {
  kind: Kind;
  title: string;
  note?: string;
  views: SectionView[];
  actions?: ReactNode;
  onSelect?: (id: string) => void;
}) {
  const { onCopyError } = useContext(DetailState);
  const panelId = useId();
  const [selected, setSelected] = useState<string>();
  const view = views.find(view => sectionViewId(view) === selected) ?? views[0];
  const hasTabs = views.length > 1;

  return <section className="tool-payload" data-kind={kind}>
    <div className="tool-payload-header">
      <span className="tool-payload-title">{title}</span>
      {note && <span className="tool-payload-note">{note}</span>}
      {(view.meta || hasTabs || view.copy || actions) && <div className="tool-payload-actions">
        {view.meta && <span>{view.meta}</span>}
        {hasTabs && <Segmented label={`${title}的展示方式`} className="section-views" tabs panelId={panelId}
          options={views.map(view => [sectionViewId(view), view.label])} value={sectionViewId(view)} onChange={id => {
            setSelected(id);
            onSelect?.(id);
          }} />}
        {view.copy && <CopyButton className="content-action btn-ghost btn-sm" label={`复制${hasTabs ? view.label : title}`}
          value={view.copy} onError={onCopyError} />}
        {actions}
      </div>}
    </div>
    {hasTabs
      ? <div id={panelId} role="tabpanel" aria-labelledby={`${panelId}-${views.indexOf(view)}`} tabIndex={0}>{view.body}</div>
      : view.body}
  </section>;
}

function message(text: string, loading = false) {
  return <p className="tool-loading tool-payload-message" role={loading ? "status" : undefined}>{text}</p>;
}

function usePayload(block?: ContentBlock, enabled = true) {
  const { payloads } = useContext(DetailState);
  const token = block?.deferred;
  const [loaded, setLoaded] = useState<{ token: string; value?: ContentBlock; error?: string }>();
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (!token || !enabled) return;
    let active = true;
    const request = payloads.get(token) ?? readToolPayload(token);
    payloads.set(token, request);
    void request.then(value => {
      if (active) setLoaded({ token, value });
    }, error => {
      if (payloads.get(token) === request) payloads.delete(token);
      if (active) setLoaded({ token, error: String(error) });
    });
    return () => { active = false; };
  }, [token, payloads, attempt, enabled]);

  const current = loaded?.token === token ? loaded : undefined;
  return {
    value: token ? current?.value : block,
    placeholder: current?.error ? <Notice tone="error" action={{ label: "重试", onClick: () => {
      setLoaded(undefined);
      setAttempt(count => count + 1);
    } }}>{current.error}</Notice> : message("正在读取…", true),
  };
}

type InputOptions<T> = {
  note?: string;
  preview?: View;
  loadOnExpand?: boolean;
  views?: (op: T, call: ContentBlock) => Views;
  actions?: ReactNode;
};

type OutputOptions<T> = {
  title?: string;
  start?: number | null;
  views?: (result: ContentBlock | undefined, op: T) => Views;
  actions?: (op: T) => ReactNode;
};

export function ToolSections<T extends ToolOp>({ item, input, output = {} }: {
  item: ToolItem;
  input?: InputOptions<T>;
  output?: OutputOptions<T> | false;
}) {
  const { content, result } = item.part;
  const [loadJson, setLoadJson] = useState(false);
  const preview = input?.preview;
  const { value: call, placeholder } = usePayload(content, !preview || !!input?.loadOnExpand || loadJson);
  const op = (call?.ops?.[item.index] ?? item.op) as T;
  const views = preview ? [preview] : (call ? input?.views?.(op, call) : undefined) ?? [];
  const json = call && JSON.stringify(call.input, null, 2);
  const raw = call ? (json === undefined ? message("无输入") : <pre className="code tool-input-code">{json}</pre>) : placeholder;

  return <>
    <Section kind="input" title="输入" note={input?.note} actions={input?.actions} onSelect={id => {
      if (id === "json") setLoadJson(true);
    }} views={[...views.filter((view): view is View => !!view), { id: "json", label: "JSON", body: raw, copy: json }]} />
    {output && <ToolOutput status={itemStatus(item)} result={result} script={item.index === 0 ? content.script_result : undefined}
      title={output.title} start={output.start} actions={output.actions?.(op)}
      views={output.views && (result => output.views!(result, op))} />}
  </>;
}

export function ToolOutput({ status, result, script, title = "输出", start = null, views, actions }: {
  status?: ContentBlock["status"];
  result?: ContentBlock;
  script?: ContentBlock;
  title?: string;
  start?: number | null;
  views?: (result?: ContentBlock) => Views;
  actions?: ReactNode;
}) {
  const output = usePayload(result);
  const scriptOutput = usePayload(script);
  const failed = status === "error";
  const heading = failed ? "错误输出" : title;
  const shown: SectionView[] = failed ? [] : (views?.(output.value) ?? []).filter((view): view is View => !!view);
  if (result) {
    const label = shown.length ? "原文" : heading;
    const original = output.value ? resultView(output.value, label, start) : { label, body: output.placeholder };
    shown.push({ ...original, id: "raw" });
  }
  if (script) {
    const label = "脚本输出";
    const decoded = scriptOutput.value && scriptView(scriptOutput.value, label);
    if (decoded) shown.push({ ...decoded, id: "script" });
    const original = scriptOutput.value
      ? resultView(scriptOutput.value, decoded ? "脚本原文" : label)
      : { label, body: scriptOutput.placeholder };
    shown.push({ ...original, id: decoded ? "script-raw" : "script" });
  }
  if (!shown.length) shown.push({ label: heading, body: message(status === "completed" || failed ? "无输出" : "等待结果") });
  return <Section kind={failed ? "error" : "output"} title={heading} actions={actions} views={shown} />;
}
