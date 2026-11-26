// Menu-bar popover: pick camera, microphone and screen, then record. Every action is one engine tool.
// The recording controls themselves live in the toolbar window (toolbar.js).
(() => {
  const { lumos } = window;
  const card = document.getElementById('card');

  const store = {
    get: (k, d) => { try { return JSON.parse(localStorage.getItem(k) ?? JSON.stringify(d)); } catch (_) { return d; } },
    set: (k, v) => localStorage.setItem(k, JSON.stringify(v)),
  };
  // source: null (main display) | {kind:'display',displayId} | {kind:'window',windowId,app,title} | {kind:'area',rect}
  let prefs = { camOn: store.get('camOn', true), micOn: store.get('micOn', true), camId: store.get('camId', null), micId: store.get('micId', null), source: store.get('source', null) };
  let devices = { cams: [], mics: [], displays: [], windows: [] };
  let open = null;            // which device list is expanded: 'cam' | 'mic' | 'display'
  let rec = { state: 'idle', elapsedUs: 0 };
  let perms = null;
  let err = null;
  let starting = false;

  const I = {
    screen: '<svg viewBox="0 0 20 20" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="15" height="10" rx="2"/><path d="M7 17h6M10 13.5V17"/></svg>',
    cam: '<svg viewBox="0 0 20 20" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="5" width="10.5" height="10" rx="2.2"/><path d="M13 9l4.5-2.5v7L13 11"/></svg>',
    mic: '<svg viewBox="0 0 20 20" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="7.5" y="2.5" width="5" height="9" rx="2.5"/><path d="M4.5 9.5a5.5 5.5 0 0011 0M10 15v2.5"/></svg>',
    chev: '<svg viewBox="0 0 12 12" width="10" height="10" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M3 4.5l3 3 3-3"/></svg>',
  };

  const nameOf = (list, id, fallback) => (list.find((d) => d.id === id) || list.find((d) => d.default) || list[0] || { name: fallback }).name;

  const clip = (s, n) => (s.length > n ? s.slice(0, n - 1) + '…' : s);
  function sourceLabel() {
    const s = prefs.source;
    if (s && s.kind === 'window') return clip(`${s.app} — ${s.title}`, 34);
    if (s && s.kind === 'area') return `Area ${Math.round(s.rect.w)} × ${Math.round(s.rect.h)}`;
    return nameOf(devices.displays, s && s.displayId, 'Full screen');
  }
  function sourceChoices() {
    const s = prefs.source;
    let html = '';
    for (const d of devices.displays) {
      const sel = !s || s.kind === 'display' ? (s && s.displayId ? d.id === s.displayId : d.main) : false;
      html += `<button class="choice ${sel ? 'sel' : ''}" role="option" data-act="pick-display" data-id="${lumos.esc(d.id)}">${lumos.esc(d.name)}</button>`;
    }
    html += `<button class="choice ${s && s.kind === 'area' ? 'sel' : ''}" role="option" data-act="pick-area">Select an area…</button>`;
    if (devices.windows.length) {
      html += '<div class="hint pad">Windows</div>';
      for (const w of devices.windows) {
        const sel = s && s.kind === 'window' && s.windowId === w.id;
        html += `<button class="choice ${sel ? 'sel' : ''}" role="option" data-act="pick-window" data-id="${w.id}">${lumos.esc(clip(`${w.app} — ${w.title}`, 40))}</button>`;
      }
    }
    return html;
  }

  function row(kind, icon, label, on, list, id, fallback) {
    const shown = kind === 'display' ? sourceLabel() : on ? nameOf(list, id, fallback) : 'Off';
    const expanded = open === kind;
    let html = `<div class="opt ${on ? '' : 'off'}">
      <button class="opt-main" data-act="expand" data-kind="${kind}" aria-expanded="${expanded}" aria-label="${label}: ${lumos.esc(shown)}">
        <span class="opt-icon">${icon}</span>
        <span class="opt-text"><span class="opt-label">${label}</span><span class="opt-value">${lumos.esc(shown)}</span></span>
        <span class="chev ${expanded ? 'up' : ''}">${I.chev}</span>
      </button>
      ${kind === 'display' ? '' : `<button class="switch" role="switch" aria-checked="${on}" aria-label="${label} on or off" data-act="toggle" data-kind="${kind}"><i></i></button>`}
    </div>`;
    if (expanded) {
      html += '<div class="choices" role="listbox">';
      if (kind === 'display') html += sourceChoices();
      if (kind !== 'display') html += `<button class="choice ${on ? '' : 'sel'}" role="option" data-act="pick" data-kind="${kind}" data-id="">Off</button>`;
      for (const d of kind === 'display' ? [] : list) {
        const sel = on && (id ? d.id === id : d.default);
        html += `<button class="choice ${sel ? 'sel' : ''}" role="option" data-act="pick" data-kind="${kind}" data-id="${lumos.esc(d.id)}">${lumos.esc(d.name)}</button>`;
      }
      if (!list.length && kind !== 'display') html += `<div class="hint pad">None found</div>`;
      html += '</div>';
    }
    return html;
  }

  function render() {
    const active = rec.state !== 'idle' && rec.state !== 'failed';
    let html = `<div class="head"><span class="logo"><i></i></span><b>Lumos</b></div>`;
    if (active) {
      html += `<div class="live"><span class="dot pulse"></span><span>${rec.state === 'paused' ? 'Paused' : rec.state === 'recording' ? 'Recording' : 'Getting ready'}</span><span class="time">${lumos.fmtClock(rec.elapsedUs)}</span></div>`;
      html += `<div class="two">`;
      if (rec.state === 'recording') html += `<button class="btn" data-act="pause">Pause</button>`;
      if (rec.state === 'paused') html += `<button class="btn" data-act="resume">Resume</button>`;
      html += `<button class="btn stop" data-act="stop">Stop and save</button></div>`;
    } else {
      if (perms && perms.screen !== 'granted') {
        html += `<div class="note">Lumos needs Screen Recording access. <button class="link" data-act="allow-screen">Allow it</button></div>`;
      }
      html += row('display', I.screen, 'Screen', true, devices.displays, null, 'Full screen');
      html += row('cam', I.cam, 'Camera', prefs.camOn, devices.cams, prefs.camId, 'No camera found');
      html += row('mic', I.mic, 'Microphone', prefs.micOn, devices.mics, prefs.micId, 'No microphone found');
      if (err) html += `<div class="note err">${lumos.esc(err)}</div>`;
      html += `<button class="go" data-act="record" ${starting ? 'disabled' : ''}><span class="dot"></span>${starting ? 'Starting…' : 'Start recording'}</button>`;
    }
    html += `<div class="foot"><button class="link" data-act="library">Library</button><button class="link" data-act="settings">Settings</button><span class="spacer"></span><button class="link" data-act="quit">Quit</button></div>`;
    card.innerHTML = html;
  }

  async function refresh() {
    const [cams, mics, displays, windows, st, p] = await Promise.all([
      lumos.call('sources.cameras').catch(() => []),
      lumos.call('sources.microphones').catch(() => []),
      lumos.call('sources.displays').catch(() => []),
      lumos.call('sources.windows').catch(() => []),
      lumos.call('recording.status').catch(() => rec),
      lumos.call('permissions.status').catch(() => null),
    ]);
    devices = { cams, mics, displays, windows };
    // A remembered window that has since closed falls back to the main display.
    if (prefs.source && prefs.source.kind === 'window' && windows.length && !windows.some((w) => w.id === prefs.source.windowId)) setPref('source', null);
    rec = st;
    perms = p;
    render();
  }

  const setPref = (k, v) => { prefs[k] = v; store.set(k, v); };
  async function syncBubble() {
    try { await lumos.invoke(prefs.camOn && cardVisible() ? 'show_window' : 'hide_window', { label: 'bubble' }); } catch (_) {}
  }
  const cardVisible = () => document.visibilityState === 'visible';

  const actions = {
    expand(b) { open = open === b.dataset.kind ? null : b.dataset.kind; render(); },
    toggle(b) {
      const k = b.dataset.kind === 'cam' ? 'camOn' : 'micOn';
      setPref(k, !prefs[k]);
      render();
      if (k === 'camOn') syncBubble();
    },
    pick(b) {
      const { kind, id } = b.dataset;
      if (!id) setPref(kind === 'cam' ? 'camOn' : 'micOn', false);
      else { setPref(kind === 'cam' ? 'camId' : 'micId', id); setPref(kind === 'cam' ? 'camOn' : 'micOn', true); }
      open = null;
      render();
      if (kind === 'cam') syncBubble();
    },
    'pick-display'(b) {
      setPref('source', { kind: 'display', displayId: b.dataset.id });
      open = null;
      render();
    },
    'pick-window'(b) {
      const w = devices.windows.find((x) => String(x.id) === b.dataset.id);
      if (w) setPref('source', { kind: 'window', windowId: w.id, app: w.app, title: w.title });
      open = null;
      render();
    },
    async 'pick-area'() {
      open = null;
      await lumos.invoke('hide_window', { label: 'menu' });
      const rect = await lumos.invoke('pick_area');
      if (rect) setPref('source', { kind: 'area', rect });
      render();
      await lumos.invoke('show_window', { label: 'menu' });
    },
    async record() {
      err = null; starting = true; render();
      try {
        const params = {
          camera: { enabled: prefs.camOn, deviceId: prefs.camOn ? prefs.camId : null },
          mic: { enabled: prefs.micOn, deviceId: prefs.micOn ? prefs.micId : null },
        };
        const s = prefs.source;
        if (s && s.kind === 'display') params.source = { kind: 'display', displayId: s.displayId };
        else if (s && s.kind === 'window') params.source = { kind: 'window', windowId: s.windowId };
        else if (s && s.kind === 'area') params.source = { kind: 'area', displayId: 'main', rect: s.rect };
        await lumos.invoke('hide_window', { label: 'menu' });
        rec = await lumos.call('recording.start', params);
      } catch (e) {
        err = lumos.errText(e);
        await lumos.invoke('show_window', { label: 'menu' });
      } finally { starting = false; render(); }
    },
    pause: () => lumos.call('recording.pause'),
    resume: () => lumos.call('recording.resume'),
    async stop() { await lumos.call('recording.stop', { wait: false }); await lumos.invoke('hide_window', { label: 'menu' }); },
    async 'allow-screen'() { await lumos.invoke('open_privacy_pane', { kind: 'screen' }); },
    async library() { await lumos.invoke('hide_window', { label: 'menu' }); await lumos.invoke('show_window', { label: 'main' }); lumos.emit('show-tab', 'library'); },
    async settings() { await lumos.invoke('hide_window', { label: 'menu' }); await lumos.invoke('show_window', { label: 'main' }); lumos.emit('show-tab', 'settings'); },
    quit: () => lumos.invoke('quit'),
  };

  card.addEventListener('click', async (ev) => {
    const b = ev.target.closest('[data-act]');
    if (!b || b.disabled) return;
    try { await actions[b.dataset.act](b); } catch (e) { err = lumos.errText(e); render(); }
  });

  lumos.onEvent((ev) => {
    if (ev.type === 'recorder.stateChanged') { rec = ev.data; render(); }
    else if (ev.type === 'recorder.tick') { rec.elapsedUs = ev.data.elapsedUs; const t = card.querySelector('.live .time'); if (t) t.textContent = lumos.fmtClock(rec.elapsedUs); }
  });

  // Each time the popover opens: reload devices and permissions, and preview the camera bubble.
  window.addEventListener('focus', () => { open = null; err = null; refresh().then(syncBubble); });
  window.addEventListener('blur', async () => {
    const st = await lumos.call('recording.status').catch(() => rec);
    if (st.state === 'idle' || st.state === 'failed') lumos.invoke('hide_window', { label: 'bubble' }).catch(() => {});
  });

  new ResizeObserver(() => {
    const r = card.getBoundingClientRect();
    lumos.invoke('set_window_size', { label: 'menu', width: Math.ceil(r.width) + 24, height: Math.ceil(r.height) + 24 }).catch(() => {});
  }).observe(card);

  render();
  refresh();
})();
