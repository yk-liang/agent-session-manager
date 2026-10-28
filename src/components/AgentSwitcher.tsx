import { ChevronDown, Layers } from "lucide-react";
import type { AgentInfo } from "../types";
import AgentIcon from "./AgentIcon";
import UiIcon from "./UiIcon";

interface Props {
  agents: AgentInfo[];
  value: string | null;
  allowAll: boolean;
  disabled: boolean;
  onChange: (id: string) => void;
}

export default function AgentSwitcher({ agents, value, allowAll, disabled, onChange }: Props) {
  return <div className="agent-switcher">
    <span className="agent-switcher-icon" aria-hidden="true">
      {value && value !== "all" ? <AgentIcon agent={value} /> : <UiIcon icon={Layers} />}
    </span>
    <select aria-label="当前 Agent" value={value ?? ""} disabled={disabled || agents.length === 0} onChange={event => onChange(event.target.value)}>
      {value === null && <option value="" disabled>未发现 Agent 数据</option>}
      {allowAll && <option value="all">全部 Agent</option>}
      {agents.map(agent => <option key={agent.id} value={agent.id} disabled={!allowAll && !agent.installed}>
        {agent.displayName}{!allowAll && !agent.installed ? "（未发现数据）" : ""}
      </option>)}
    </select>
    <UiIcon icon={ChevronDown} className="agent-switcher-chevron" />
  </div>;
}
