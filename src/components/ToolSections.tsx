import { useContext, useEffect, useId, useState, type ReactNode } from "react";
import { itemStatus, type ToolItem, type ToolStatus } from "../activity";
import { readToolPayload } from "../api";
import type { ContentBlock, ToolOp } from "../types";
import { shortPath } from "../utils";
import CopyButton from "./CopyButton";
import { DetailState } from "./DetailState";
import Notice from "./Notice";
import { jsonResult, plainResultText, resultHasAttachment, resultText, resultView, scriptView } from "./ResultViews";
import Segmented from "./Segmented";
import { payloadText, terminalText, TextPreview, textView, type View } from "./ToolContent";

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
  // 复制按钮跟随整个小节，不随页签出现或消失，切换条位置才稳定。
  const copyable = views.some(view => view.copy);

  return <section className="tool-payload" data-kind={kind}>
    <div className="tool-payload-header">
      <span className="tool-payload-title">{title}</span>
      {note && <span className="tool-payload-note">{note}</span>}
      {(view.meta || hasTabs || copyable || actions) && <div className="tool-payload-actions">
        {view.meta && <span>{view.meta}</span>}
        {hasTabs && <Segmented label={`${title}的展示方式`} className="section-views" tabs panelId={panelId}
          options={views.map(view => [sectionViewId(view), view.label])} value={sectionViewId(view)} onChange={id => {
            setSelected(id);
            onSelect?.(id);
          }} />}
        {copyable && <CopyButton className="content-action btn-ghost btn-sm" label={`复制${hasTabs ? view.label : title}`}
          value={view.copy ?? ""} disabled={!view.copy} onError={onCopyError} />}
        {actions}
      </div>}
    </div>
    {hasTabs
      ? <div key={sectionViewId(view)} id={panelId} role="tabpanel" aria-labelledby={`${panelId}-${views.indexOf(view)}`} tabIndex={0}>{view.body}</div>
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
  sourceLabel?: string;
};

type OutputOptions<T> = {
  title?: string;
  start?: number | null;
  views?: (result: ContentBlock | undefined, op: T) => Views;
  actions?: (op: T) => ReactNode;
  preferScript?: boolean;
  terminal?: boolean;
  hideEmpty?: boolean;
};

function isDuplicatePlain(view: View, shown: SectionView[]) {
  return !!view.plain && view.copy !== undefined && shown.some(other => other.plain && other.copy === view.copy);
}

function cleanBodyView(native: ContentBlock | undefined, title: string, start: number | null, terminal: boolean): View | undefined {
  if (!native) return;
  let body: string | undefined;
  if (native.output !== undefined) {
    body = terminal ? terminalText(native.output) : native.output;
  } else if (terminal) {
    const raw = plainResultText(native);
    const clean = raw !== undefined ? terminalText(raw) : undefined;
    if (clean !== undefined && clean !== raw) body = clean;
  }
  if (body === undefined) return;
  return body.trim() ? textView(title, body, start) : { label: title, body: message("无输出"), plain: true };
}

export function ToolSections<T extends ToolOp>({ item, input, output = {} }: {
  item: ToolItem;
  input?: InputOptions<T>;
  output?: OutputOptions<T> | false;
}) {
  const { content, result } = item.part;
  const [loadSource, setLoadSource] = useState(false);
  const preview = input?.preview;
  const { value: call, placeholder } = usePayload(content, !preview || !!input?.loadOnExpand || loadSource);
  const op = (call?.ops?.[item.index] ?? item.op) as T;
  const views = preview ? [preview] : (call ? input?.views?.(op, call) : undefined) ?? [];
  const empty = call?.input == null || typeof call.input === "object" && !Array.isArray(call.input) && !Object.keys(call.input).length;
  const source = call && !empty ? payloadText(call.input) : undefined;
  const sourceLabel = input?.sourceLabel ?? (typeof call?.input === "string" ? "原文" : "JSON");
  const raw = call ? (source === undefined ? message("无输入") : <TextPreview label={sourceLabel} text={source}>
    {text => <pre className="code tool-input-code">{text}</pre>}
  </TextPreview>) : placeholder;

  return <>
    <Section kind="input" title="输入" note={input?.note} actions={input?.actions} onSelect={id => {
      if (id === "source") setLoadSource(true);
    }} views={[...views.filter((view): view is View => !!view), { id: "source", label: sourceLabel, body: raw, copy: source }]} />
    {output && <ToolOutput status={itemStatus(item)} result={result} script={item.index === 0 ? content.script_result : undefined}
      title={output.title} start={output.start} actions={output.actions?.(op)} preferScript={output.preferScript}
      terminal={output.terminal} hideEmpty={output.hideEmpty}
      views={output.views && (result => output.views!(result, op))} />}
  </>;
}

export function ToolOutput({ status, result, script, title = "输出", start = null, views, actions, preferScript, terminal = false, hideEmpty = false }: {
  status?: ToolStatus;
  result?: ContentBlock;
  script?: ContentBlock;
  title?: string;
  start?: number | null;
  views?: (result?: ContentBlock) => Views;
  actions?: ReactNode;
  preferScript?: boolean;
  terminal?: boolean;
  hideEmpty?: boolean;
}) {
  const output = usePayload(result);
  const scriptOutput = usePayload(script);
  const failed = status === "error" || status === "denied";
  const heading = failed ? "错误输出" : title;
  const native = output.value;
  const nativeHasAttachment = resultHasAttachment(native);
  const nativeViews = (views?.(native) ?? []).filter((view): view is View => !!view);
  const json = !nativeViews.length && jsonResult(native);
  if (json) nativeViews.push(json);
  const nativeText = native && plainResultText(native)?.trim();
  const nativeEmpty = !!native && !nativeViews.length && !nativeHasAttachment && !nativeText;
  const rawNativeEmpty = !!native && !nativeHasAttachment && !resultText(native);
  const scriptBlock = scriptOutput.value;
  const scriptHasAttachment = resultHasAttachment(scriptBlock);
  const decoded = scriptBlock && scriptView(scriptBlock, "脚本输出", terminal);
  const scriptText = decoded?.copy?.trim();
  const duplicate = !!scriptText && !scriptHasAttachment && !nativeHasAttachment && scriptText === nativeText;
  const semanticScript = decoded && !duplicate && (scriptText || !decoded.copy) ? { ...decoded, id: "script" } : undefined;
  // 原生结果为空时（包装脚本里的命令只把输出交给了脚本），脚本输出就是唯一的结果。
  const scriptFirst = !!(semanticScript && scriptText && (nativeEmpty || preferScript && !failed && !nativeHasAttachment && scriptText.length > (nativeText?.length ?? 0)));
  if (!nativeViews.length && !nativeHasAttachment && !scriptFirst) {
    const body = cleanBodyView(native, title, start, terminal);
    if (body) nativeViews.push(body);
  }
  const saved = native?.output_file;
  if (hideEmpty && !failed && !script && !saved) {
    if (!result) return null;
    if (native) {
      const hasContent = nativeHasAttachment || nativeViews.some(view => !view.plain || !!view.copy?.trim()) || (!nativeViews.length && !!nativeText);
      if (!hasContent) return null;
    }
  }
  const shown: SectionView[] = [...nativeViews];
  if (scriptFirst && semanticScript) shown.unshift(semanticScript);
  if (result && !(scriptFirst && rawNativeEmpty)) {
    const label = scriptFirst && !nativeViews.length ? "原生结果" : shown.length ? "原文" : heading;
    const original: View = native ? resultView(native, label, shown.length ? null : start) : { label, body: output.placeholder };
    if (!isDuplicatePlain(original, shown)) shown.push({ ...original, id: "raw" });
  }
  if (script) {
    const label = decoded ? "脚本原文" : "脚本输出";
    if (semanticScript && !scriptFirst && !isDuplicatePlain(semanticScript, shown)) shown.push(semanticScript);
    const original = scriptBlock
      ? resultView(scriptBlock, label)
      : { label, body: scriptOutput.placeholder };
    if (!isDuplicatePlain(original, shown)) shown.push({ ...original, id: decoded ? "script-raw" : "script" });
  }
  if (!shown.length) shown.push({ label: heading, body: message(status === "completed" || failed || status === "cancelled" ? "无输出" : "等待结果") });
  return <Section kind={failed ? "error" : "output"} title={heading} actions={actions} views={shown}
    note={saved && `完整输出另存在 ${shortPath(saved)}`} />;
}
