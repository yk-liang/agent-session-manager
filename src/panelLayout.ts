export type Panel = "sidebar" | "sessions";
export type PanelWidths = Record<Panel, number>;

export const DEFAULT_WIDTHS: PanelWidths = { sidebar: 220, sessions: 320 };
export const MIN_WIDTHS = { sidebar: 180, sessions: 260, detail: 360 };
export const DIVIDER_WIDTH = 8;
const STORAGE_KEY = "agent-session-manager.panel-widths.v1";

export function readPanelWidths(): PanelWidths {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null");
    const valid = (panel: Panel) => typeof saved?.[panel] === "number"
      && Number.isFinite(saved[panel]) && saved[panel] >= MIN_WIDTHS[panel];
    return {
      sidebar: valid("sidebar") ? saved.sidebar : DEFAULT_WIDTHS.sidebar,
      sessions: valid("sessions") ? saved.sessions : DEFAULT_WIDTHS.sessions,
    };
  } catch {
    return { ...DEFAULT_WIDTHS };
  }
}

export function savePanelWidths(widths: PanelWidths) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(widths));
  } catch {
    // 存储不可用时仍允许本次调整，避免拖拽被浏览器存储权限打断。
  }
}

export function fitPanelWidths(preferred: PanelWidths, width: number, hasSessions: boolean): PanelWidths {
  if (width === 0) return preferred;
  const available = width - DIVIDER_WIDTH * (hasSessions ? 2 : 1) - MIN_WIDTHS.detail;
  if (!hasSessions) {
    return { ...preferred, sidebar: Math.max(MIN_WIDTHS.sidebar, Math.min(preferred.sidebar, available)) };
  }

  // 只压缩超出最小宽度的部分；窗口缩放不改写用户偏好，放大后可自动恢复。
  const sidebarExtra = preferred.sidebar - MIN_WIDTHS.sidebar;
  const sessionsExtra = preferred.sessions - MIN_WIDTHS.sessions;
  const extra = sidebarExtra + sessionsExtra;
  const room = Math.max(0, available - MIN_WIDTHS.sidebar - MIN_WIDTHS.sessions);
  const scale = extra > 0 ? Math.min(1, room / extra) : 1;
  return {
    sidebar: MIN_WIDTHS.sidebar + Math.floor(sidebarExtra * scale),
    sessions: MIN_WIDTHS.sessions + Math.floor(sessionsExtra * scale),
  };
}

export function maxPanelWidth(panel: Panel, widths: PanelWidths, width: number, hasSessions: boolean): number {
  const otherWidth = hasSessions ? widths[panel === "sidebar" ? "sessions" : "sidebar"] : 0;
  return Math.max(MIN_WIDTHS[panel], width - DIVIDER_WIDTH * (hasSessions ? 2 : 1) - otherWidth - MIN_WIDTHS.detail);
}

export function resizePanel(panel: Panel, requested: number, widths: PanelWidths, width: number, hasSessions: boolean): PanelWidths {
  return { ...widths, [panel]: Math.round(Math.max(MIN_WIDTHS[panel], Math.min(requested, maxPanelWidth(panel, widths, width, hasSessions)))) };
}
