// Shared by every window. The UI never touches devices or files: each action is one engine tool.
(() => {
  const T = window.__TAURI__;
  const invoke = (cmd, args) => T.core.invoke(cmd, args || {});

  window.lumos = {
    // Call an engine tool by name, e.g. lumos.call('recording.start', { ... }). Rejects with { code, message }.
    call: (method, params) => invoke('call', { method, params: params || {} }),
    invoke,
    // Engine events: { type, ts, data }.
    onEvent: (fn) => T.event.listen('engine-event', (e) => fn(e.payload)),
    emit: (name, payload) => T.event.emit(name, payload),
    on: (name, fn) => T.event.listen(name, (e) => fn(e.payload)),
    fileUrl: (path) => T.core.convertFileSrc(path),
    fmtClock(us) {
      const s = Math.max(0, Math.floor((us || 0) / 1e6));
      const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), r = s % 60;
      const mm = String(m).padStart(2, '0'), rr = String(r).padStart(2, '0');
      return h ? `${h}:${mm}:${rr}` : `${mm}:${rr}`;
    },
    fmtBytes(n) {
      if (n < 1e6) return `${Math.max(1, Math.round(n / 1e3))} KB`;
      if (n < 1e9) return `${(n / 1e6).toFixed(1)} MB`;
      return `${(n / 1e9).toFixed(2)} GB`;
    },
    esc: (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c])),
    errText: (e) => (e && e.message) || String(e),
    basename: (p) => String(p || '').split('/').pop(),
  };
})();
