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
  counts?: ReadonlyMap<string, number | null>;
  onOpen?: () => void;
}

const allAgents = { id: "all", name: "全部 Agent", icon: <UiIcon icon={Layers} /> };

export default function AgentSwitcher({ agents, value, allowAll, disabled, onChange, counts, onOpen }: Props) {
  const options = agents.map(agent => ({
    id: agent.id, name: agent.displayName, icon: <AgentIcon agent={agent.id} />,
    count: counts?.get(agent.id),
  }));
  return <Dropdown className="agent-switcher" label="当前 Agent" value={value}
    options={allowAll ? [{ ...allAgents, count: counts?.get("all") }, ...options] : options}
    countUnit={counts ? allowAll ? "条请求记录" : "个会话" : undefined}
    disabled={disabled} placeholder="未找到本地 Agent 数据" onChange={onChange} onOpen={onOpen} />;
}
