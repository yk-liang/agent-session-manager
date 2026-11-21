import type { SessionEvent } from "./types";

export function bareModelId(model: string) {
  return model.slice(model.lastIndexOf("/") + 1).trim();
}

export function qualifiedModelId(model: string, provider?: string | null) {
  return provider && !model.includes("/") ? `${provider}/${model}` : model;
}

function assistantModel(event: SessionEvent) {
  return event.type === "assistant" ? event.message?.model : undefined;
}

export function historicalModels(events: SessionEvent[]): string[] {
  const models = new Set<string>();
  for (let index = events.length - 1; index >= 0; index--) {
    const model = assistantModel(events[index]);
    if (model) models.add(model);
  }
  return [...models];
}

export function eventModelTitle(event: SessionEvent): string | undefined {
  const model = assistantModel(event);
  if (model) return `模型：${model}`;
}
