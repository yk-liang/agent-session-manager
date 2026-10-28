import qoder from "../assets/agents/qoder.svg";
import claude from "../assets/agents/claude.png";
import codex from "../assets/agents/codex.png";
import opencode from "../assets/agents/opencode.png";
import pi from "../assets/agents/pi.svg";

const icons: Record<string, string> = { qoder, claude, codex, opencode, pi };

/** 图标与旁边的 Agent 名称共同呈现，避免屏幕阅读器重复朗读。 */
export default function AgentIcon({ agent }: { agent: string }) {
  const src = icons[agent];
  return src ? (
    <img
      className={`agent-icon agent-icon-${agent}`}
      src={src}
      alt=""
      aria-hidden="true"
    />
  ) : null;
}
