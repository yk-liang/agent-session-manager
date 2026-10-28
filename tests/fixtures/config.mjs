// 仅供界面回归的内存配置；所有路径和内容均为虚构，不读写真实 CLI 配置。
export const configFixtures = {
  qoder: [
    { key: "settings.json", content: '{\n  "model": "work-model",\n  "model_provider": "company"\n}\n' },
    { key: "mcp.json", content: '{\n  "mcpServers": {"filesystem": {"command": "node"}}\n}\n' },
    { key: ".models/default", content: null },
  ],
  claude: [{ key: "settings.json", content: JSON.stringify({ model: "claude-demo", ...Object.fromEntries(Array.from({ length: 90 }, (_, i) => [`setting${i}`, i === 40 ? "long-value-".repeat(100) : `示例 ${i}`])) }, null, 2) + "\n" }],
  codex: [{ key: "config.toml", content: '# keep this comment\nmodel = "codex-demo"\nmodel_provider = "company"\n\n[mcp_servers.docs]\ncommand = "docs-server"\n' }],
  opencode: [{ key: "opencode.jsonc", content: null }, { key: "opencode.json", content: '{\n  "model": "demo/model"\n}\n' }],
  pi: [{ key: "settings.json", content: null }],
};

export function configMockBootstrap(input) {
  const fixtures = input.fixtures ?? input;
  const native = input.native ?? false;
  const oldInvoke = window.__TAURI_INTERNALS__?.invoke;
  const names = { qoder: "Qoder CLI", claude: "Claude Code", codex: "Codex", opencode: "OpenCode", pi: "Pi" };
  const state = {
    configs: Object.fromEntries(Object.entries(fixtures).map(([agent, files]) => [agent, files.map((file) => ({ ...file, label: file.key, path: `/Users/demo/.${agent}/${file.key}`, present: file.content !== null }))])),
    profiles: {}, backups: {}, calls: [], writes: [], sequence: 0,
    holdReads: false, pendingReads: [], failNextRead: false,
    holdWrites: false, pendingWrites: [], slowValidation: false,
    callbacks: {}, listeners: {}, nextListener: 0, guard: { dirty: false, busy: false }, exitCount: 0,
  };
  if (native) {
    window.isTauri = true;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    state.emit = (event) => {
      for (const [id, listener] of Object.entries(state.listeners)) if (listener.event === event) {
        state.callbacks[listener.handler]({ event, id: Number(id), payload: null });
      }
    };
  }
  state.profiles.qoder = [
    { name: "工作", agent: "qoder", createdAt: 1792022400000, files: structuredClone(state.configs.qoder) },
    { name: "个人", agent: "qoder", createdAt: 1792022600000, files: [{ ...state.configs.qoder[0], content: '{\n  "model": "personal-model"\n}\n', present: true }] },
  ];
  window.__CONFIG_TEST__ = state;
  window.__CONFIG_BY_AGENT__ = Object.fromEntries(Object.entries(state.configs).map(([agent, files]) => [agent, { agent, files }]));
  window.__INVOKE_LOG__ ??= [];
  const revision = (files) => JSON.stringify(files.map((f) => [f.key, f.path, f.present, f.content]));
  const fileRevision = (file) => JSON.stringify([file.present, file.content]);
  const validate = (key, content) => {
    try {
      if (key.endsWith(".json")) { const parsed = JSON.parse(content); if (!parsed || Array.isArray(parsed) || typeof parsed !== "object") throw Error("root must be an object"); }
      // 格式解析的严格性由 Rust 测试验证；这里驱动界面的通过、错误与竞态状态。
      if (key.endsWith(".jsonc")) JSON.parse(content.replace(/\/\/[^\n]*/g, "").replace(/,\s*([}\]])/g, "$1"));
      if (key.endsWith(".toml") && /model\s*=\s*\[\s*$/.test(content)) throw Error("unclosed array");
      return { valid: true, error: null };
    } catch { return { valid: false, error: "语法错误，第 1 行第 2 列：示例错误" }; }
  };
  const targetFiles = (agent, kind, target) => {
    const current = state.configs[agent];
    if (kind === "profile") {
      const profile = (state.profiles[agent] ?? []).find((p) => p.name === target);
      if (!profile) throw Error("未找到配置方案");
      return current.map((file) => { const saved = profile.files.find((f) => f.key === file.key); return { ...file, content: saved?.content ?? null, present: saved?.content != null }; });
    }
    const backup = (state.backups[agent] ?? []).find((b) => b.id === target);
    if (!backup) throw Error("未找到备份");
    return structuredClone(backup.snapshot);
  };
  const summary = (profile, agent) => ({ ...profile, files: state.configs[agent].map((file) => ({ key: file.key, label: file.label, present: profile.files.some((f) => f.key === file.key && f.present) })) });
  const workspace = (agent) => {
    const files = state.configs[agent];
    const profiles = state.profiles[agent] ?? [];
    const first = files.find((f) => f.present)?.content ?? "";
    let parsed = {}; try { parsed = JSON.parse(first); } catch {}
    const model = parsed.model ?? first.match(/^model\s*=\s*"([^"]+)"/m)?.[1] ?? null;
    const serverFile = files.find((f) => f.key === "mcp.json");
    let servers = []; try { servers = Object.keys(JSON.parse(serverFile?.content ?? "{}").mcpServers ?? {}); } catch {}
    return { agent, revision: revision(files),
      files: files.map((file) => ({ ...file, revision: fileRevision(file), validation: file.present ? validate(file.key, file.content) : { valid: true, error: null } })),
      profiles: profiles.map((p) => summary(p, agent)), matchingProfiles: profiles.filter((p) => revision(targetFiles(agent, "profile", p.name)) === revision(files)).map((p) => p.name),
      overview: { model, provider: parsed.model_provider ?? null, endpoint: null, mcpServers: servers },
      backups: (state.backups[agent] ?? []).map(({ snapshot, ...backup }) => backup), warnings: [],
    };
  };
  const preview = (agent, kind, target) => {
    const current = state.configs[agent];
    const next = targetFiles(agent, kind, target);
    for (const file of next) if (file.present && !validate(file.key, file.content).valid) throw Error("目标配置语法错误");
    return { kind, target, title: kind === "profile" ? `应用配置「${target}」` : "恢复操作前备份", currentRevision: revision(current), targetRevision: revision(next), files: next.map((after) => {
      const before = current.find((f) => f.key === after.key);
      const action = before.content === after.content && before.present === after.present ? "unchanged" : !after.present ? "remove" : !before.present ? "create" : "replace";
      return { key: after.key, label: after.label, path: after.path, action, before: before.content, after: after.content };
    }) };
  };
  const backup = (agent, files, label) => {
    state.backups[agent] ??= [];
    state.backups[agent].unshift({ id: `history-${++state.sequence}`, createdAt: Date.now(), label, files: files.map((f) => f.key), legacy: false, snapshot: structuredClone(files) });
  };
  const commands = new Set(["get_config_workspace", "validate_config_content", "update_config_file", "save_profile", "delete_profile", "preview_config_change", "apply_config_change"]);
  window.__TAURI_INTERNALS__ = { ...(window.__TAURI_INTERNALS__ ?? {}), transformCallback: (callback) => {
    if (!native) return callback;
    const id = ++state.nextListener; state.callbacks[id] = callback; return id;
  }, invoke: async (command, args = {}) => {
    if (native) {
      if (command === "plugin:event|listen") { const id = ++state.nextListener; state.listeners[id] = args; return id; }
      if (command === "plugin:event|unlisten") { delete state.listeners[args.eventId]; return null; }
      if (command === "set_config_guard") { state.guard = structuredClone(args); return null; }
      if (command === "confirm_config_exit") {
        if (state.guard.busy) throw Error("配置操作正在处理中，请稍后再退出");
        state.guard = { dirty: false, busy: false }; state.exitCount++; return null;
      }
      if (command === "watch_sessions") return null;
    }
    if (!commands.has(command)) {
      if (oldInvoke) return oldInvoke(command, args);
      if (command === "list_agents") return Object.entries(names).map(([id, displayName]) => ({ id, displayName, installed: true, supportsFork: false, dataDir: `/Users/demo/.${id}`, deletePolicy: "trash" }));
      if (command === "catalog") return { projects: [], sessions: [], warnings: [] };
      throw Error(`Unexpected command: ${command}`);
    }
    state.calls.push({ command, args: structuredClone(args) });
    window.__INVOKE_LOG__.push({ cmd: command, args: structuredClone(args) });
    const files = state.configs[args.agent];
    if (command === "get_config_workspace") {
      const result = structuredClone(workspace(args.agent));
      if (state.holdReads) await new Promise((resolve, reject) => state.pendingReads.push({ resolve, reject }));
      if (state.failNextRead) { state.failNextRead = false; throw Error("mock config reload failure"); }
      return result;
    }
    if (command === "validate_config_content") {
      if (state.slowValidation && args.content.includes("slow")) await new Promise((r) => setTimeout(r, 700));
      return validate(args.key, args.content);
    }
    if (command === "update_config_file") {
      if (state.holdWrites) await new Promise((resolve) => state.pendingWrites.push(resolve));
      const file = files.find((f) => f.key === args.key);
      if (fileRevision(file) !== args.expectedRevision) throw Error("此文件已被外部修改，本次未覆盖磁盘内容");
      if (!validate(args.key, args.content).valid) throw Error("配置语法错误");
      backup(args.agent, [file], `编辑 ${file.key}`);
      file.content = args.content; file.present = true;
      state.writes.push(structuredClone(args)); return null;
    }
    if (command === "save_profile") {
      if (revision(files) !== args.expectedRevision) throw Error("配置文件已被外部修改");
      state.profiles[args.agent] ??= [];
      if (state.profiles[args.agent].some((p) => p.name === args.name)) throw Error("方案已存在");
      const profile = { name: args.name, agent: args.agent, createdAt: Date.now(), files: structuredClone(files) };
      state.profiles[args.agent].push(profile); return summary(profile, args.agent);
    }
    if (command === "delete_profile") { state.profiles[args.agent] = (state.profiles[args.agent] ?? []).filter((p) => p.name !== args.name); return null; }
    if (command === "preview_config_change") return structuredClone(preview(args.agent, args.kind, args.target));
    if (command === "apply_config_change") {
      const fresh = preview(args.agent, args.kind, args.target);
      if (args.expectedCurrent !== fresh.currentRevision) throw Error("配置文件已被外部修改，请重新读取差异");
      if (args.expectedTarget !== fresh.targetRevision) throw Error("目标配置已变化，请重新读取差异");
      const next = targetFiles(args.agent, args.kind, args.target);
      backup(args.agent, files.filter((file) => next.some((f) => f.key === file.key)), args.kind === "profile" ? `应用配置「${args.target}」` : "恢复备份");
      for (const file of next) Object.assign(files.find((f) => f.key === file.key), file);
      state.writes.push(structuredClone(args)); return null;
    }
  } };
}
