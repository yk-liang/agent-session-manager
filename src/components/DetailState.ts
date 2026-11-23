import { createContext } from "react";
import type { ContentBlock } from "../types";

/** 会话正文共享的阅读状态：卡片展开、已读取的工具正文和显示选项。 */
export const DetailState = createContext({
  expanded: new Map<string, boolean>(),
  payloads: new Map<string, Promise<ContentBlock>>(),
  markdown: false,
  cwd: null as string | null,
  onCopyError: (_error: unknown) => {},
});
