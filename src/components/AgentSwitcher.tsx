import { Layers } from "lucide-react";
import type { AgentInfo } from "../types";
import AgentIcon from "./AgentIcon";
import Dropdown from "./Dropdown";
import UiIcon from "./UiIcon";

interface Props {
  agents: AgentInfo[];
  value: string | null;
  allowAll: boolean;
  disabled: boolean;
  onChange: (id: string) => void;
}

const allAgents = { id: "all", name: "全部 Agent", icon: <UiIcon icon={Layers} /> };

export default function AgentSwitcher({ agents, value, allowAll, disabled, onChange }: Props) {
  const options = agents.map(agent => ({
    id: agent.id, name: agent.displayName, icon: <AgentIcon agent={agent.id} />,
  }));
  return <Dropdown className="agent-switcher" label="当前 Agent" value={value}
    options={allowAll ? [allAgents, ...options] : options} disabled={disabled} placeholder="未发现 Agent 数据" onChange={onChange} />;
}
