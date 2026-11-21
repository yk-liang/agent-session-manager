import anthropic from "@lobehub/icons-static-svg/icons/anthropic.svg";
import azure from "@lobehub/icons-static-svg/icons/azure-color.svg";
import bailian from "@lobehub/icons-static-svg/icons/bailian-color.svg";
import bedrock from "@lobehub/icons-static-svg/icons/bedrock-color.svg";
import cerebras from "@lobehub/icons-static-svg/icons/cerebras-color.svg";
import claude from "@lobehub/icons-static-svg/icons/claude-color.svg";
import cohere from "@lobehub/icons-static-svg/icons/cohere-color.svg";
import copilot from "@lobehub/icons-static-svg/icons/copilot-color.svg";
import deepseek from "@lobehub/icons-static-svg/icons/deepseek-color.svg";
import doubao from "@lobehub/icons-static-svg/icons/doubao-color.svg";
import fireworks from "@lobehub/icons-static-svg/icons/fireworks-color.svg";
import gemini from "@lobehub/icons-static-svg/icons/gemini-color.svg";
import gemma from "@lobehub/icons-static-svg/icons/gemma-color.svg";
import google from "@lobehub/icons-static-svg/icons/google-color.svg";
import grok from "@lobehub/icons-static-svg/icons/grok.svg";
import groq from "@lobehub/icons-static-svg/icons/groq.svg";
import kimi from "@lobehub/icons-static-svg/icons/kimi-color.svg";
import meta from "@lobehub/icons-static-svg/icons/meta-color.svg";
import minimax from "@lobehub/icons-static-svg/icons/minimax-color.svg";
import mistral from "@lobehub/icons-static-svg/icons/mistral-color.svg";
import moonshot from "@lobehub/icons-static-svg/icons/moonshot.svg";
import nova from "@lobehub/icons-static-svg/icons/nova-color.svg";
import ollama from "@lobehub/icons-static-svg/icons/ollama.svg";
import openai from "@lobehub/icons-static-svg/icons/openai.svg";
import opencode from "@lobehub/icons-static-svg/icons/opencode.svg";
import openrouter from "@lobehub/icons-static-svg/icons/openrouter-color.svg";
import perplexity from "@lobehub/icons-static-svg/icons/perplexity-color.svg";
import qwen from "@lobehub/icons-static-svg/icons/qwen-color.svg";
import siliconcloud from "@lobehub/icons-static-svg/icons/siliconcloud-color.svg";
import together from "@lobehub/icons-static-svg/icons/together-color.svg";
import volcengine from "@lobehub/icons-static-svg/icons/volcengine-color.svg";
import xai from "@lobehub/icons-static-svg/icons/xai.svg";
import zai from "@lobehub/icons-static-svg/icons/zai.svg";
import zhipu from "@lobehub/icons-static-svg/icons/zhipu-color.svg";
import { bareModelId } from "../models";

const mono = new Set([anthropic, grok, groq, moonshot, ollama, openai, opencode, xai, zai]);
const providers = new Map(Object.entries({
  anthropic, azure, bedrock, cerebras, cohere, deepseek, fireworks, google, groq, minimax,
  mistral, moonshot, ollama, openai, opencode, openrouter, perplexity, qwen, siliconcloud, together, volcengine, xai, zai, zhipu,
  "amazon-bedrock": bedrock, "azure-openai": azure, "github-copilot": copilot, "google-vertex": google,
  alibaba: bailian, bailian, dashscope: bailian, aliyun: bailian, "alibaba-cloud": bailian, "qwen-portal": qwen,
  fireworksai: fireworks, "kimi-for-coding": kimi, "minimax-cn": minimax, moonshotai: moonshot, "moonshotai-cn": moonshot,
  siliconflow: siliconcloud, togetherai: together, "volcengine-plan": volcengine, ark: volcengine,
  "x-ai": xai, "z.ai": zai, "z-ai": zai, "zhipuai": zhipu, "zhipuai-coding-plan": zhipu, "zai-coding-plan": zai,
}));

const models = new Map(Object.entries({
  claude, gemini, gemma, qwen, deepseek, kimi, minimax, grok, mistral, doubao, nova,
  gpt: openai, chatgpt: openai, codex: openai, qwq: qwen, qvq: qwen, moonshot: kimi,
  glm: zhipu, chatglm: zhipu, llama: meta,
  mixtral: mistral, codestral: mistral, pixtral: mistral, devstral: mistral, ministral: mistral,
  command: cohere, aya: cohere, sonar: perplexity,
}));

export function modelIconSource(model: string) {
  const id = bareModelId(model).toLowerCase();
  if (/^o[1-9](?=[\d._:\s-]|$)/.test(id)) return openai;
  const family = /^[a-z]+(?=[\d._:\s-]|$)/.exec(id)?.[0];
  return family ? models.get(family) : undefined;
}

export default function BrandIcon({ src, className, monochrome }: { src?: string; className?: string; monochrome?: boolean }) {
  if (!src) return null;
  const classes = ["brand-icon", className, (monochrome || mono.has(src)) && "brand-icon-mono"].filter(Boolean).join(" ");
  return <img className={classes} src={src} alt="" aria-hidden="true" />;
}

export function ModelIcon({ id }: { id: string }) {
  return <BrandIcon src={modelIconSource(id)} />;
}

export function ProviderIcon({ provider }: { provider: string }) {
  return <BrandIcon src={providers.get(provider.trim().toLowerCase().replace(/\s+/g, "-"))} />;
}
