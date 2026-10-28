import { Component, lazy, memo, Suspense, type ReactNode } from "react";
import HighlightedText from "./HighlightedText";

const MarkdownContent = lazy(() => import("./MarkdownContent"));
interface Props { text: string; query: string; markdown: boolean; children?: ReactNode; fallback?: ReactNode }

// 日志中的异常格式或资源加载失败只回退当前卡片，不能让整段历史白屏。
class MarkdownBoundary extends Component<Props, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  render() {
    const { text, query } = this.props;
    return this.state.failed ? (this.props.fallback ?? <HighlightedText text={text} query={query} />) : this.props.children;
  }
}

export default memo(function MessageText({ text, query, markdown, fallback }: Props) {
  if (!markdown) return <>{fallback ?? <HighlightedText text={text} query={query} />}</>;
  return <MarkdownBoundary key={text} text={text} query={query} markdown={markdown} fallback={fallback}>
    <Suspense fallback={<div data-markdown-pending>{fallback ?? <HighlightedText text={text} query={query} />}</div>}>
      <MarkdownContent text={text} query={query} fallback={fallback} />
    </Suspense>
  </MarkdownBoundary>;
});
