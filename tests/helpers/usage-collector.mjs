function createCollector(agents, collect = async () => false, resolveCallback = callback => callback) {
  let status = { sequence: 0, revision: 0, updatedAt: null, syncing: [], recollecting: [], progress: null, failedAgents: [], unavailable: false };
  let running;
  let listenerId = -1;
  const listeners = new Map();
  const scans = [];
  const update = (patch, changed = false) => {
    status = { ...status, ...patch, sequence: status.sequence + 1 };
    if (changed) {
      status.revision++;
      status.updatedAt = Date.now();
    }
    for (const [id, handler] of listeners) resolveCallback(handler)?.({ event: 'usage-sync-status', id, payload: structuredClone(status) });
  };
  const check = () => {
    if (running) return running;
    update({ syncing: [...agents] });
    running = Promise.resolve().then(async () => {
      for (const agent of agents) {
        scans.push(agent);
        let changed = false;
        const failedAgents = new Set(status.failedAgents);
        try {
          changed = await collect(agent) ?? true;
          failedAgents.delete(agent);
        } catch {
          failedAgents.add(agent);
        }
        update({ failedAgents: [...failedAgents], progress: null, syncing: status.syncing.filter(id => id !== agent) }, changed);
      }
    }).finally(() => running = undefined);
    return running;
  };
  return {
    scans,
    check,
    changed: () => update({}, true),
    progress: progress => update({ progress }),
    bridge: invoke => (command, args = {}) => {
      if (command === 'usage_sync_status') return Promise.resolve(structuredClone(status));
      if (command === 'check_usage_now') { void check(); return Promise.resolve(); }
      if (command === 'plugin:event|listen' && args.event === 'usage-sync-status') {
        const id = listenerId--;
        listeners.set(id, args.handler);
        return Promise.resolve(id);
      }
      if (command === 'plugin:event|unlisten' && listeners.delete(args.eventId)) return Promise.resolve();
      return invoke(command, args);
    },
  };
}

export function addUsageInitScript(page, initialize, data = null) {
  return page.addInitScript({ content: `(${initialize})(${JSON.stringify(data)}, ${createCollector});` });
}
