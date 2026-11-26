// Menu-bar popover: pick what to record (display, window or area), the camera, the microphone and
// system audio, then record. Every action is one engine tool. The recording controls themselves live
// in the toolbar window (toolbar.js).
(() => {
  const { lumos } = window;
  const card = document.getElementById('card');

  const store = {
    get: (k, d) => { try { return JSON.parse(localStorage.getItem(k) ?? JSON.stringify(d)); } catch (_) { return d; } },
    set: (k, v) => localStorage.setItem(k, JSON.stringify(v)),
  };
  // source: null (main display) | {kind:'display',displayId} | {kind:'window',windowId,app,title} | {kind:'area',rect}
  // lastDisplayId / lastWindow remember the choice inside each tile, so switching tiles keeps it.
  let prefs = {
    camOn: store.get('camOn', true), micOn: store.get('micOn', true), camId: store.get('camId', null), micId: store.get('micId', null),
    source: store.get('source', null), lastDisplayId: store.get('lastDisplayId', null), lastWindow: store.get('lastWindow', null),
  };
  // System audio and both volumes live in Settings, so the popover and the Settings window always agree.
  let audio = { systemAudio: false, micGainDb: 0, systemGainDb: -6 };
  let devices = { cams: [], mics: [], displays: [], windows: [] };
  let open = null;            // expanded list: 'display' | 'window' | 'cam' | 'mic'
  let volOpen = null;         // expanded volume slider: 'mic' | 'sys'
  let rec = { state: 'idle', elapsedUs: 0 };
  let perms = null;
  let err = null;
  let starting = false;

  const svg = (body, size = 18) => `<svg viewBox="0 0 20 20" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
  const I = {
    display: svg('<rect x="2.5" y="3.5" width="15" height="10" rx="2"/><path d="M7 17h6M10 13.5V17"/>', 24),
    window: svg('<rect x="2.5" y="3.5" width="15" height="13" rx="2"/><path d="M2.5 7h15"/><path d="M5 5.25h.01M7 5.25h.01M9 5.25h.01"/>', 24),
    area: svg('<path d="M3 7V4.5A1.5 1.5 0 014.5 3H7M13 3h2.5A1.5 1.5 0 0117 4.5V7M17 13v2.5a1.5 1.5 0 01-1.5 1.5H13M7 17H4.5A1.5 1.5 0 013 15.5V13"/>', 22),
    cam: svg('<rect x="2.5" y="5" width="10.5" height="10" rx="2.2"/><path d="M13 9l4.5-2.5v7L13 11"/>', 20),
    mic: svg('<rect x="7.5" y="2.5" width="5" height="9" rx="2.5"/><path d="M4.5 9.5a5.5 5.5 0 0011 0M10 15v2.5"/>', 20),
    speaker: svg('<path d="M3 7.5h3l4-3.5v12l-4-3.5H3z"/><path d="M13.5 7.2a4 4 0 010 5.6M15.8 5a7 7 0 010 10"/>', 20),
    sliders: svg('<path d="M4 6h12M4 14h12"/><circle cx="8" cy="6" r="2" fill="#fff"/><circle cx="12.5" cy="14" r="2" fill="#fff"/>', 18),
    library: svg('<rect x="3" y="3" width="6" height="6" rx="1.3"/><rect x="11" y="3" width="6" height="6" rx="1.3"/><rect x="3" y="11" width="6" height="6" rx="1.3"/><rect x="11" y="11" width="6" height="6" rx="1.3"/>', 18),
    gear: svg('<circle cx="10" cy="10" r="2.6"/><path d="M10 2.5v2M10 15.5v2M2.5 10h2M15.5 10h2M4.7 4.7l1.4 1.4M13.9 13.9l1.4 1.4M4.7 15.3l1.4-1.4M13.9 6.1l1.4-1.4"/>', 18),
    power: svg('<path d="M10 2.8v6.4"/><path d="M5.6 5.4a6.2 6.2 0 108.8 0"/>', 18),
    chev: '<svg viewBox="0 0 12 12" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 4.5l3 3 3-3"/></svg>',
  };

  const esc = lumos.esc;
  const clip = (s, n) => (s.length > n ? s.slice(0, n - 1) + '…' : s);
  const nameOf = (list, id, fallback) => (list.find((d) => d.id === id) || list.find((d) => d.default) || list[0] || { name: fallback }).name;
  const mainDisplay = () => devices.displays.find((d) => d.main) || devices.displays[0];
  const displayName = (id) => (devices.displays.find((d) => d.id === id) || mainDisplay() || { name: 'Main display' }).name;
  const kind = () => (prefs.source && prefs.source.kind) || 'display';
  const setPref = (k, v) => { prefs[k] = v; store.set(k, v); };

  // Volume: the engine stores a gain in dB; people think in percent. 0 dB = 100%.
  const GAIN_MIN = -30, GAIN_MAX = 6;
  const pct = (db) => `${Math.round(100 * Math.pow(10, db / 20))}%`;
  const fill = (db) => `${(100 * (db - GAIN_MIN)) / (GAIN_MAX - GAIN_MIN)}%`;

  // ---- pieces ----------------------------------------------------------------

  function splitTile(k, icon, label, sub) {
    const sel = kind() === k;
    return `<div class="tile ${sel ? 'sel' : ''}">
      <button class="tile-main" data-act="src-${k}" aria-pressed="${sel}">${icon}<span class="tile-name">${label}</span><span class="tile-sub">${esc(sub)}</span></button>
      <button class="tile-more" data-act="expand" data-kind="${k}" aria-expanded="${open === k}" aria-label="Choose a ${label.toLowerCase()}"><span class="chev">${I.chev}</span></button>
    </div>`;
  }

  function sources() {
    const s = prefs.source;
    const win = prefs.lastWindow;
    const area = s && s.kind === 'area' ? `${Math.round(s.rect.w)} × ${Math.round(s.rect.h)}` : 'Drag to select';
    let html = '<div class="sources">';
    html += splitTile('display', I.display, 'Display', displayName(kind() === 'display' && s ? s.displayId : prefs.lastDisplayId));
    html += splitTile('window', I.window, 'Window', win ? win.app : 'Choose a window');
    html += `<div class="tile wide ${kind() === 'area' ? 'sel' : ''}"><button class="tile-main" data-act="pick-area" aria-pressed="${kind() === 'area'}">${I.area}<span class="tile-name">Area</span><span class="tile-sub">${esc(area)}</span></button></div>`;
    html += '</div>';
    if (open === 'display') {
      html += '<div class="choices" role="listbox" aria-label="Displays">';
      const cur = kind() === 'display' && s ? s.displayId : prefs.lastDisplayId;
      for (const d of devices.displays) {
        const sel = kind() === 'display' && (cur ? d.id === cur : d === mainDisplay());
        html += `<button class="choice ${sel ? 'sel' : ''}" role="option" aria-selected="${sel}" data-act="pick-display" data-id="${esc(d.id)}">${esc(d.name)}</button>`;
      }
      if (!devices.displays.length) html += '<div class="hint pad">None found</div>';
      html += '</div>';
    }
    if (open === 'window') {
      html += '<div class="choices" role="listbox" aria-label="Windows">';
      for (const w of devices.windows) {
        const sel = kind() === 'window' && s.windowId === w.id;
        html += `<button class="choice ${sel ? 'sel' : ''}" role="option" aria-selected="${sel}" data-act="pick-window" data-id="${w.id}">${esc(clip(`${w.app} — ${w.title}`, 44))}</button>`;
      }
      if (!devices.windows.length) html += '<div class="hint pad">No windows found</div>';
      html += '</div>';
    }
    return html;
  }

  function volume(k) {
    const db = k === 'mic' ? audio.micGainDb : audio.systemGainDb;
    const label = k === 'mic' ? 'Microphone volume' : 'System audio volume';
    return `<div class="vol" id="vol-${k}"><label for="gain-${k}">Volume</label>
      <input type="range" id="gain-${k}" data-gain="${k}" min="${GAIN_MIN}" max="${GAIN_MAX}" step="1" value="${db}" aria-label="${label}" aria-valuetext="${pct(db)}" style="--fill:${fill(db)}">
      <output for="gain-${k}">${pct(db)}</output></div>`;
  }

  // A device row: name (opens the device list), optional volume button, and an On/Off pill.
  function device(k, icon, label, on, name, list, id) {
    const expandable = !!list;
    const main = expandable
      ? `<button class="dev-main" data-act="expand" data-kind="${k}" aria-expanded="${open === k}" aria-label="${label}: ${esc(name)}" title="Choose a ${label.toLowerCase()}">${icon}<span class="dev-name">${esc(name)}</span></button>`
      : `<div class="dev-main">${icon}<span class="dev-name">${esc(name)}</span></div>`;
    const vol = k === 'cam' ? '' : `<button class="icon-btn" data-act="vol" data-kind="${k}" aria-expanded="${volOpen === k}" aria-controls="vol-${k}" aria-label="${label} volume" title="${label} volume">${I.sliders}</button>`;
    let html = `<div class="dev ${on ? '' : 'off'}">${main}${vol}
      <button class="pill-switch" role="switch" aria-checked="${on}" aria-label="${label}" data-act="toggle" data-kind="${k}">${on ? 'On' : 'Off'}</button></div>`;
    if (expandable && open === k) {
      html += `<div class="choices" role="listbox" aria-label="${label}">`;
      html += `<button class="choice ${on ? '' : 'sel'}" role="option" aria-selected="${!on}" data-act="pick" data-kind="${k}" data-id="">Off</button>`;
      for (const d of list) {
        const sel = on && (id ? d.id === id : d.default);
        html += `<button class="choice ${sel ? 'sel' : ''}" role="option" aria-selected="${sel}" data-act="pick" data-kind="${k}" data-id="${esc(d.id)}">${esc(d.name)}</button>`;
      }
      if (!list.length) html += '<div class="hint pad">None found</div>';
      html += '</div>';
    }
    if (volOpen === k) html += volume(k);
    return html;
  }

  function render() {
    const active = rec.state !== 'idle' && rec.state !== 'failed';
    let html = `<div class="head"><span class="logo" aria-hidden="true"><i></i></span><b>Lumos</b><span class="spacer"></span>
      <button class="icon-btn" data-act="library" aria-label="Library" title="Library">${I.library}</button>
      <button class="icon-btn" data-act="settings" aria-label="Settings" title="Settings">${I.gear}</button>
      <button class="icon-btn" data-act="quit" aria-label="Quit Lumos" title="Quit Lumos">${I.power}</button></div>`;
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
      html += sources();
      html += device('cam', I.cam, 'Camera', prefs.camOn, prefs.camOn ? nameOf(devices.cams, prefs.camId, 'No camera found') : 'No camera', devices.cams, prefs.camId);
      html += device('mic', I.mic, 'Microphone', prefs.micOn, prefs.micOn ? nameOf(devices.mics, prefs.micId, 'No microphone found') : 'No microphone', devices.mics, prefs.micId);
      html += device('sys', I.speaker, 'System audio', audio.systemAudio, 'Record system audio', null, null);
      if (err) html += `<div class="note err" role="alert">${esc(err)}</div>`;
      html += `<button class="go" data-act="record" ${starting ? 'disabled' : ''}><span class="dot"></span>${starting ? 'Starting…' : 'Start recording'}</button>`;
    }
    card.innerHTML = html;
  }

  async function refresh() {
    const [cams, mics, displays, windows, st, p, settings] = await Promise.all([
      lumos.call('sources.cameras').catch(() => []),
      lumos.call('sources.microphones').catch(() => []),
      lumos.call('sources.displays').catch(() => []),
      lumos.call('sources.windows').catch(() => []),
      lumos.call('recording.status').catch(() => rec),
      lumos.call('permissions.status').catch(() => null),
      lumos.call('settings.get').catch(() => null),
    ]);
    devices = { cams, mics, displays, windows };
    // A remembered window that has since closed falls back to the main display.
    if (windows.length && prefs.lastWindow && !windows.some((w) => w.id === prefs.lastWindow.windowId)) setPref('lastWindow', null);
    if (kind() === 'window' && windows.length && !windows.some((w) => w.id === prefs.source.windowId)) setPref('source', null);
    if (settings && settings.audio) {
      const a = settings.audio;
      audio = { systemAudio: !!a.systemAudio, micGainDb: a.micGainDb ?? 0, systemGainDb: a.systemGainDb ?? -6 };
    }
    rec = st;
    perms = p;
    render();
  }

  async function syncBubble() {
    try { await lumos.invoke(prefs.camOn && cardVisible() ? 'show_window' : 'hide_window', { label: 'bubble' }); } catch (_) {}
  }
  const cardVisible = () => document.visibilityState === 'visible';
  const saveAudio = (patch) => lumos.call('settings.set', { patch });

  function chooseWindow(w) {
    const v = { kind: 'window', windowId: w.id, app: w.app, title: w.title };
    setPref('source', v);
    setPref('lastWindow', v);
  }

  const actions = {
    expand(b) { open = open === b.dataset.kind ? null : b.dataset.kind; render(); },
    vol(b) { volOpen = volOpen === b.dataset.kind ? null : b.dataset.kind; render(); },
    'src-display'() {
      const id = prefs.lastDisplayId || (mainDisplay() && mainDisplay().id);
      setPref('source', id ? { kind: 'display', displayId: id } : null);
      open = null;
      render();
    },
    'src-window'() {
      const w = prefs.lastWindow;
      if (w && devices.windows.some((x) => x.id === w.windowId)) { setPref('source', w); open = null; }
      else open = 'window';
      render();
    },
    async toggle(b) {
      const k = b.dataset.kind;
      if (k === 'sys') {
        const on = !audio.systemAudio;
        audio.systemAudio = on;
        render();
        try { await saveAudio({ 'audio.systemAudio': on }); } catch (e) { audio.systemAudio = !on; throw e; }
        return;
      }
      const key = k === 'cam' ? 'camOn' : 'micOn';
      setPref(key, !prefs[key]);
      render();
      if (k === 'cam') syncBubble();
    },
    pick(b) {
      const { kind: k, id } = b.dataset;
      if (!id) setPref(k === 'cam' ? 'camOn' : 'micOn', false);
      else { setPref(k === 'cam' ? 'camId' : 'micId', id); setPref(k === 'cam' ? 'camOn' : 'micOn', true); }
      open = null;
      render();
      if (k === 'cam') syncBubble();
    },
    'pick-display'(b) {
      setPref('source', { kind: 'display', displayId: b.dataset.id });
      setPref('lastDisplayId', b.dataset.id);
      open = null;
      render();
    },
    'pick-window'(b) {
      const w = devices.windows.find((x) => String(x.id) === b.dataset.id);
      if (w) chooseWindow(w);
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
          systemAudio: audio.systemAudio,
        };
        const s = prefs.source;
        if (s && s.kind === 'display') params.source = { kind: 'display', displayId: s.displayId };
        else if (s && s.kind === 'window') params.source = { kind: 'window', windowId: s.windowId };
        else if (s && s.kind === 'area') params.source = { kind: 'area', displayId: 'main', rect: s.rect };
        // The toolbar's restart button starts again with these.
        localStorage.setItem('lastStart', JSON.stringify(params));
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

  // Volume sliders: update the label while dragging, save on release. Gains apply when the recording is saved.
  card.addEventListener('input', (ev) => {
    const el = ev.target.closest('[data-gain]');
    if (!el) return;
    const db = Number(el.value);
    el.style.setProperty('--fill', fill(db));
    el.setAttribute('aria-valuetext', pct(db));
    el.nextElementSibling.textContent = pct(db);
  });
  card.addEventListener('change', async (ev) => {
    const el = ev.target.closest('[data-gain]');
    if (!el) return;
    const db = Number(el.value);
    const key = el.dataset.gain === 'mic' ? 'micGainDb' : 'systemGainDb';
    const before = audio[key];
    audio[key] = db;
    try { await saveAudio({ [`audio.${key}`]: db }); } catch (e) { audio[key] = before; err = lumos.errText(e); render(); }
  });

  lumos.onEvent((ev) => {
    if (ev.type === 'recorder.stateChanged') { rec = ev.data; render(); }
    else if (ev.type === 'recorder.tick') { rec.elapsedUs = ev.data.elapsedUs; const t = card.querySelector('.live .time'); if (t) t.textContent = lumos.fmtClock(rec.elapsedUs); }
  });

  // Each time the popover opens: reload devices, permissions and settings, and preview the camera bubble.
  window.addEventListener('focus', () => { open = null; volOpen = null; err = null; refresh().then(syncBubble); });
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
