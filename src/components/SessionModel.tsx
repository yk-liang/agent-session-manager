import type { SessionMeta } from "../types";

export function ModelLabel({ id, name = id, badge = false }: { id: string; name?: string; badge?: boolean }) {
  const tooltip = name === id ? id : `${name}\n模型 ID：${id}`;
  return <span className={`model-label${badge ? " badge" : ""}`} title={tooltip}>{name}</span>;
}

/** 列表与会话信息共用名称和原始 ID，长名称交给布局处理。 */
export default function SessionModel({ session, badge = false }: { session: SessionMeta; badge?: boolean }) {
  if (!session.model) return null;
  return <ModelLabel id={session.model} name={session.modelName || session.model} badge={badge} />;
}
