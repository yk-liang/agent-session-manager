import qoder from "@lobehub/icons-static-svg/icons/qoder-color.svg";
import claude from "@lobehub/icons-static-svg/icons/claudecode-color.svg";
import codex from "@lobehub/icons-static-svg/icons/codex-color.svg";
import opencode from "@lobehub/icons-static-svg/icons/opencode.svg";
import pi from "@lobehub/icons-static-svg/icons/pi.svg";
import antigravity from "../assets/agents/antigravity.svg";
import BrandIcon from "./BrandIcon";

const icons = new Map(Object.entries({ qoder, claude, codex, opencode, pi, antigravity }));

export default function AgentIcon({ agent, inline = false }: { agent: string; inline?: boolean }) {
  return <BrandIcon src={icons.get(agent)} className={inline ? undefined : "agent-icon"} monochrome={agent === "pi"} />;
}
