const KEY = "agent-session-manager.markdown.v1";

export function readMarkdownPreference(): boolean {
  try { return localStorage.getItem(KEY) === "true"; } catch { return false; }
}
export function saveMarkdownPreference(enabled: boolean) {
  try { localStorage.setItem(KEY, String(enabled)); } catch {
    // 存储不可用时只保留本次阅读选择，不影响开关和正文展示。
  }
}
