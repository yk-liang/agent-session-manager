import type { SessionEvent } from "./types";

export interface HistoricalModel { id: string; name: string }
/** 每条助手消息带有自己的模型；去重仅供信息面板展示，不合并或重排消息。 */
export function historicalModels(events: SessionEvent[]): HistoricalModel[] {
  const models = new Map<string, HistoricalModel>();
  for (const event of events) {
    const id = event.type === "assistant" ? event.message?.model : undefined;
    if (id) models.set(id, { id, name: event.message?.modelName || id });
  }
  return [...models.values()];
}
export function eventModelTitle(event: SessionEvent): string | undefined {
  if (event.type !== "assistant" || !event.message?.model) return undefined;
  const { model, modelName } = event.message;
  return modelName && modelName !== model ? `模型：${modelName}\n模型 ID：${model}` : `模型：${model}`;
}
