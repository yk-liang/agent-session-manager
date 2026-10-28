import { useMemo, useState, type MouseEvent, type ReactNode } from "react";
import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { canRenderMarkdown, externalMarkdownLink, highlightMarkdown } from "../markdown";
import HighlightedText from "./HighlightedText";

export default function MarkdownContent({ text, query, fallback }: { text: string; query: string; fallback?: ReactNode }) {
  const rendered = useMemo(() => canRenderMarkdown(text, query), [text, query]);
  const [linkError, setLinkError] = useState("");
  const components = useMemo<Components>(() => {
    const follow = (event: MouseEvent<HTMLAnchorElement>, href: string) => {
      event.preventDefault();
      if (href.startsWith("#")) {
        try {
          const id = decodeURIComponent(href.slice(1));
          const body = event.currentTarget.closest(".markdown-body");
          Array.from(body?.querySelectorAll<HTMLElement>("[id]") ?? []).find((node) => node.id === id)?.scrollIntoView({ block: "nearest" });
        } catch { /* Malformed fragment: keep the current conversation open. */ }
      } else if (externalMarkdownLink(href)) {
        void openUrl(href).catch(() => setLinkError("无法打开链接"));
      }
    };
    return {
      a: ({ href, children }) => href && (externalMarkdownLink(href) || href.startsWith("#"))
        ? <a href={href} title={href} onClick={(event) => follow(event, href)}>{children}</a>
        : <span title={href}>{children}</span>,
      // 历史记录中的远程图片不自动请求网络；保留图片说明与可主动打开的地址。
      img: ({ src, alt }) => src && externalMarkdownLink(src)
        ? <a className="markdown-image" href={src} title={src} onClick={(event) => follow(event, src)}>图片：{alt || "查看图片"}</a>
        : <span className="markdown-image">图片：{alt || src || "图片"}</span>,
      table: ({ children }) => <div className="markdown-table"><table>{children}</table></div>,
    };
  }, []);
  if (!rendered) return <>{fallback ?? <HighlightedText text={text} query={query} />}</>;
  return <div className="markdown-body">
    <Markdown remarkPlugins={[remarkGfm]} rehypePlugins={[[highlightMarkdown, { query }]]} components={components}>{text}</Markdown>
    {linkError && <span className="markdown-link-error" role="alert">{linkError}</span>}
  </div>;
}
