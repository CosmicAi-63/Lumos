// Menu-bar popover: pick what to record (display, window or area), the camera, the microphone and
// system audio, then record. Every action is one engine tool. The recording controls themselves live
// in the toolbar window (toolbar.js).
//
// Lists and volume sliders open as accordions (transitions.dev "Accordion expand"): the markup is built
// once by render(), and opening, toggling and picking update that markup in place so CSS can animate it.
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
  let audio = { systemAudio: false, micGainDb: 0, systemGainDb: 0 };
  let devices = { cams: [], mics: [], displays: [], windows: [] };
  let open = null;            // the one open accordion: 'display' | 'window' | 'cam' | 'mic' | 'sys'
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
    // Header icons: Regen Icons (MIT) layout-grid, file-text, settings and power, on a 24px grid.
    library: '<svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 3H9.5A1 1 0 0 1 10.5 4V9.5A1 1 0 0 1 9.5 10.5H4A1 1 0 0 1 3 9.5V4A1 1 0 0 1 4 3ZM14.5 3H20A1 1 0 0 1 21 4V9.5A1 1 0 0 1 20 10.5H14.5A1 1 0 0 1 13.5 9.5V4A1 1 0 0 1 14.5 3ZM4 13.5H9.5A1 1 0 0 1 10.5 14.5V20A1 1 0 0 1 9.5 21H4A1 1 0 0 1 3 20V14.5A1 1 0 0 1 4 13.5ZM14.5 13.5H20A1 1 0 0 1 21 14.5V20A1 1 0 0 1 20 21H14.5A1 1 0 0 1 13.5 20V14.5A1 1 0 0 1 14.5 13.5Z"/></svg>',
    notes: '<svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12.38 3L6.5 3A1.5 1.5 0 0 0 5 4.5L5 19.5A1.5 1.5 0 0 0 6.5 21L17.5 21A1.5 1.5 0 0 0 19 19.5L19 9.62A1.5 1.5 0 0 0 18.56 8.56L13.44 3.44A1.5 1.5 0 0 0 12.38 3ZM13 4L13 8A1 1 0 0 0 14 9L18 9M9 13L15 13M9 17L15 17"/></svg>',
    gear: '<svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M19.7 9.7L20.66 9.86A1 1 0 0 1 21.5 10.85L21.5 13.15A1 1 0 0 1 20.66 14.14L19.7 14.3A.9 .9 0 0 0 19.04 15.59L19.68 16.86A1 1 0 0 1 19.49 18.01L18.01 19.49A1 1 0 0 1 16.86 19.68L15.59 19.04A.9 .9 0 0 0 14.3 19.7L14.14 20.66A1 1 0 0 1 13.15 21.5L10.85 21.5A1 1 0 0 1 9.86 20.66L9.7 19.7A.9 .9 0 0 0 8.41 19.04L7.14 19.68A1 1 0 0 1 5.99 19.49L4.51 18.01A1 1 0 0 1 4.32 16.86L4.96 15.59A.9 .9 0 0 0 4.3 14.3L3.34 14.14A1 1 0 0 1 2.5 13.15L2.5 10.85A1 1 0 0 1 3.34 9.86L4.3 9.7A.9 .9 0 0 0 4.96 8.41L4.32 7.14A1 1 0 0 1 4.51 5.99L5.99 4.51A1 1 0 0 1 7.14 4.32L8.41 4.96A.9 .9 0 0 0 9.7 4.3L9.86 3.34A1 1 0 0 1 10.85 2.5L13.15 2.5A1 1 0 0 1 14.14 3.34L14.3 4.3A.9 .9 0 0 0 15.59 4.96L16.86 4.32A1 1 0 0 1 18.01 4.51L19.49 5.99A1 1 0 0 1 19.68 7.14L19.04 8.41A.9 .9 0 0 0 19.7 9.7ZM9 12a3 3 0 1 0 6 0a3 3 0 1 0 -6 0Z"/></svg>',
    power: '<svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17.66 7.34A8 8 0 1 1 6.34 7.34M12 2L12 11"/></svg>',
    vol: svg('<path d="M3 7.5h3l4-3.5v12l-4-3.5H3z"/><path d="M13.5 7.2a4 4 0 010 5.6"/>', 16),
    // Symmetric about the viewBox centre so the scaleY flip lands exactly on the "^".
    chev: '<span class="t-acc-chevron" aria-hidden="true"><svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6.5L8 10.5L12 6.5"/></svg></span>',
    check: '<svg class="check" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3.5 8.5l3 3 6-7"/></svg>',
  };

  const esc = lumos.esc;
  const clip = (s, n) => (s.length > n ? s.slice(0, n - 1) + '…' : s);
  const nameOf = (list, id, fallback) => (list.find((d) => d.id === id) || list.find((d) => d.default) || list[0] || { name: fallback }).name;
  const mainDisplay = () => devices.displays.find((d) => d.main) || devices.displays[0];
  const displayName = (id) => (devices.displays.find((d) => d.id === id) || mainDisplay() || { name: 'Main display' }).name;
  const kind = () => (prefs.source && prefs.source.kind) || 'display';
  const setPref = (k, v) => { prefs[k] = v; store.set(k, v); };
  const $ = (sel) => card.querySelector(sel);
  const $$ = (sel) => card.querySelectorAll(sel);

  // Volume: the engine stores a gain in dB; people think in percent. 0 dB = 100%.
  const GAIN_MIN = -30, GAIN_MAX = 6;
  const pct = (db) => `${Math.round(100 * Math.pow(10, db / 20))}%`;
  const fill = (db) => `${(100 * (db - GAIN_MIN)) / (GAIN_MAX - GAIN_MIN)}%`;

  // ---- what each piece shows ------------------------------------------------

  const devOn = (k) => (k === 'cam' ? prefs.camOn : k === 'mic' ? prefs.micOn : audio.systemAudio);
  function devName(k) {
    if (k === 'cam') return prefs.camOn ? nameOf(devices.cams, prefs.camId, 'No camera found') : 'No camera';
    if (k === 'mic') return prefs.micOn ? nameOf(devices.mics, prefs.micId, 'No microphone found') : 'No microphone';
    return 'Record system audio';
  }
  const devLabel = { cam: 'Camera', mic: 'Microphone', sys: 'System audio' };
  function tileSub(k) {
    const s = prefs.source;
    if (k === 'display') return displayName(kind() === 'display' && s ? s.displayId : prefs.lastDisplayId);
    if (k === 'window') return prefs.lastWindow ? prefs.lastWindow.app : 'Choose a window';
    return s && s.kind === 'area' ? `${Math.round(s.rect.w)} × ${Math.round(s.rect.h)}` : 'Drag to select';
  }
  // Which item in a list is selected: '' is "Off".
  function selectedId(list) {
    const s = prefs.source;
    if (list === 'display') {
      if (kind() !== 'display') return null;
      const id = (s && s.displayId) || prefs.lastDisplayId || (mainDisplay() && mainDisplay().id);
      return id == null ? null : String(id);
    }
    if (list === 'window') return kind() === 'window' ? String(s.windowId) : null;
    if (list === 'cam') return prefs.camOn ? String(prefs.camId || (devices.cams.find((d) => d.default) || devices.cams[0] || {}).id || '') : '';
    return prefs.micOn ? String(prefs.micId || (devices.mics.find((d) => d.default) || devices.mics[0] || {}).id || '') : '';
  }

  // ---- markup ---------------------------------------------------------------

  const choice = (act, list, id, label, i) =>
    `<button class="choice" role="option" data-act="${act}" data-list="${list}" data-id="${esc(id)}" style="--i:${i}"><span class="choice-name">${esc(label)}</span>${I.check}</button>`;

  function volume(k, i) {
    const db = k === 'mic' ? audio.micGainDb : audio.systemGainDb;
    const label = k === 'mic' ? 'Microphone volume' : 'System audio volume';
    return `<div class="vol" style="--i:${i}">${I.vol}
      <input type="range" id="gain-${k}" data-gain="${k}" min="${GAIN_MIN}" max="${GAIN_MAX}" step="1" value="${db}" aria-label="${label}" aria-valuetext="${pct(db)}" style="--fill:${fill(db)}">
      <output for="gain-${k}">${pct(db)}</output></div>`;
  }

  function panelBody(k) {
    let html = '';
    let i = 0;
    if (k === 'display') {
      for (const d of devices.displays) html += choice('pick-display', 'display', d.id, d.name, i++);
      if (!devices.displays.length) html += '<div class="hint pad">None found</div>';
    } else if (k === 'window') {
      for (const w of devices.windows) html += choice('pick-window', 'window', w.id, clip(`${w.app} — ${w.title}`, 44), i++);
      if (!devices.windows.length) html += '<div class="hint pad">No windows found</div>';
    } else if (k === 'cam' || k === 'mic') {
      html += choice('pick', k, '', 'Off', i++);
      for (const d of k === 'cam' ? devices.cams : devices.mics) html += choice('pick', k, d.id, d.name, i++);
      if (k === 'mic') html += volume('mic', i++);
    } else {
      html += volume('sys', i++);
      html += `<div class="hint pad" style="--i:${i++}">Sound from apps, like a video or a call.</div>`;
    }
    return html;
  }

  const listLabel = { display: 'Displays', window: 'Windows', cam: 'Cameras', mic: 'Microphones' };
  const panel = (k, cls = '') =>
    `<div class="t-acc-panel ${cls}" id="panel-${k}"><div class="t-acc-panel-inner"><div class="panel-body" ${listLabel[k] ? `role="listbox" aria-label="${listLabel[k]}"` : ''}>${panelBody(k)}</div></div></div>`;

  function splitTile(k, icon, label) {
    return `<div class="tile" data-tile="${k}">
      <button class="tile-main" data-act="src-${k}" aria-pressed="false">${icon}<span class="tile-name">${label}</span><span class="tile-sub"></span></button>
      <button class="tile-more t-acc-head" data-act="expand" data-kind="${k}" aria-expanded="false" aria-controls="panel-${k}" aria-label="Choose a ${label.toLowerCase()}">${I.chev}</button>
    </div>`;
  }

  function device(k, icon) {
    return `<div class="dev t-acc" data-acc="${k}" data-open="false">
      <div class="dev-row">
        <button class="dev-main t-acc-head" data-act="expand" data-kind="${k}" aria-expanded="false" aria-controls="panel-${k}" title="${k === 'sys' ? 'System audio volume' : `Choose a ${devLabel[k].toLowerCase()}`}">${icon}<span class="dev-name"></span>${I.chev}</button>
        <button class="pill-switch" role="switch" aria-checked="false" aria-label="${devLabel[k]}" data-act="toggle" data-kind="${k}"><span class="pill-on" aria-hidden="true">On</span><span class="pill-off" aria-hidden="true">Off</span></button>
      </div>
      ${panel(k)}
    </div>`;
  }

  function render() {
    const active = rec.state !== 'idle' && rec.state !== 'failed';
    let html = `<div class="head"><span class="logo" aria-hidden="true"><i></i></span><b>Lumos</b><span class="spacer"></span>
      <button class="icon-btn notes-btn" data-act="notes" aria-label="Show speaker notes" title="Speaker notes (never recorded)">${I.notes}</button>
      <button class="icon-btn lib" data-act="library" aria-label="Library" title="Library">${I.library}</button>
      <button class="icon-btn gear" data-act="settings" aria-label="Settings" title="Settings">${I.gear}</button>
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
      html += `<div class="src-block">
        <div class="sources">
          ${splitTile('display', I.display, 'Display')}
          ${splitTile('window', I.window, 'Window')}
          <div class="tile wide" data-tile="area"><button class="tile-main" data-act="pick-area" aria-pressed="false">${I.area}<span class="tile-name">Area</span><span class="tile-sub"></span></button></div>
        </div>
        <div class="t-acc src-acc" data-acc="display" data-open="false">${panel('display', 'src-panel')}</div>
        <div class="t-acc src-acc" data-acc="window" data-open="false">${panel('window', 'src-panel')}</div>
      </div>`;
      html += device('cam', I.cam);
      html += device('mic', I.mic);
      html += device('sys', I.speaker);
      html += `<div class="note err" role="alert" ${err ? '' : 'hidden'}>${esc(err || '')}</div>`;
      html += `<button class="go" data-act="record" ${starting ? 'disabled' : ''}><span class="dot"></span><span class="go-label">${starting ? 'Starting…' : 'Start recording'}</span></button>`;
    }
    card.innerHTML = html;
    if (!active) { update(); setOpen(open, true); }
  }

  // ---- in-place updates (these are what animate) -----------------------------

  function update() {
    for (const k of ['display', 'window', 'area']) {
      const t = $(`[data-tile="${k}"]`);
      if (!t) continue;
      const sel = kind() === k;
      t.classList.toggle('sel', sel);
      t.querySelector('.tile-main').setAttribute('aria-pressed', String(sel));
      t.querySelector('.tile-sub').textContent = tileSub(k);
    }
    for (const k of ['cam', 'mic', 'sys']) {
      const d = $(`[data-acc="${k}"]`);
      if (!d) continue;
      const on = devOn(k);
      d.classList.toggle('off', !on);
      d.querySelector('.dev-name').textContent = devName(k);
      d.querySelector('.dev-main').setAttribute('aria-label', `${devLabel[k]}: ${devName(k)}`);
      d.querySelector('.pill-switch').setAttribute('aria-checked', String(on));
    }
    for (const list of ['display', 'window', 'cam', 'mic']) {
      const id = selectedId(list);
      for (const c of $$(`.choice[data-list="${list}"]`)) {
        const sel = id !== null && c.dataset.id === id;
        c.classList.toggle('sel', sel);
        c.setAttribute('aria-selected', String(sel));
      }
    }
    const e = $('.note.err');
    if (e) { e.hidden = !err; e.textContent = err || ''; }
  }

  // The window is sized to the card. While a panel grows, hold the window at the final height up
  // front so the card never draws past the window's edge for a frame.
  let holdH = 0, holdTimer = 0;
  const accMs = () => parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--acc-expand')) || 320;
  function sizeWindow() {
    const r = card.getBoundingClientRect();
    lumos.invoke('set_window_size', { label: 'menu', width: Math.ceil(r.width) + 24, height: Math.ceil(Math.max(r.height, holdH)) + 24 }).catch(() => {});
  }

  function setOpen(k, instant = false) {
    const prev = open;
    open = k;
    let grow = 0;
    for (const el of $$('[data-acc]')) {
      const want = el.dataset.acc === k;
      const was = el.dataset.open === 'true';
      if (want === was) continue;
      const p = el.querySelector('.t-acc-panel');
      if (want) grow += p.querySelector('.t-acc-panel-inner').scrollHeight;
      else grow -= p.getBoundingClientRect().height;
      if (instant) el.classList.add('no-anim');
      el.dataset.open = String(want);
    }
    for (const h of $$('.t-acc-head')) h.setAttribute('aria-expanded', String(h.dataset.kind === k));
    for (const t of $$('[data-tile]')) t.classList.toggle('expanded', t.dataset.tile === k);
    if (instant) {
      void card.offsetHeight;
      for (const el of $$('.no-anim')) el.classList.remove('no-anim');
      return;
    }
    if (grow > 0 && prev !== k) {
      holdH = card.getBoundingClientRect().height + grow;
      sizeWindow();
      clearTimeout(holdTimer);
      holdTimer = setTimeout(() => { holdH = 0; sizeWindow(); }, accMs() + 60);
    }
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
      audio = { systemAudio: !!a.systemAudio, micGainDb: a.micGainDb ?? 0, systemGainDb: a.systemGainDb ?? 0 };
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

  // Close the open list after a pick, letting the check mark land first.
  const closeSoon = () => setTimeout(() => setOpen(null), 180);

  const actions = {
    expand(b) { setOpen(open === b.dataset.kind ? null : b.dataset.kind); },
    'src-display'() {
      const id = prefs.lastDisplayId || (mainDisplay() && mainDisplay().id);
      setPref('source', id ? { kind: 'display', displayId: id } : null);
      update();
      if (open === 'window') setOpen(null);
    },
    'src-window'() {
      const w = prefs.lastWindow;
      if (w && devices.windows.some((x) => x.id === w.windowId)) {
        setPref('source', w);
        update();
        if (open === 'display') setOpen(null);
      } else setOpen('window');
    },
    async toggle(b) {
      const k = b.dataset.kind;
      if (k === 'sys') {
        const on = !audio.systemAudio;
        audio.systemAudio = on;
        update();
        try { await saveAudio({ 'audio.systemAudio': on }); } catch (e) { audio.systemAudio = !on; update(); throw e; }
        return;
      }
      const key = k === 'cam' ? 'camOn' : 'micOn';
      setPref(key, !prefs[key]);
      update();
      if (k === 'cam') syncBubble();
    },
    pick(b) {
      const { list: k, id } = b.dataset;
      if (!id) setPref(k === 'cam' ? 'camOn' : 'micOn', false);
      else { setPref(k === 'cam' ? 'camId' : 'micId', id); setPref(k === 'cam' ? 'camOn' : 'micOn', true); }
      update();
      // The microphone panel also holds its volume, so it stays open after a pick.
      if (k === 'cam') { closeSoon(); syncBubble(); }
    },
    'pick-display'(b) {
      setPref('source', { kind: 'display', displayId: b.dataset.id });
      setPref('lastDisplayId', b.dataset.id);
      update();
      closeSoon();
    },
    'pick-window'(b) {
      const w = devices.windows.find((x) => String(x.id) === b.dataset.id);
      if (w) {
        const v = { kind: 'window', windowId: w.id, app: w.app, title: w.title };
        setPref('source', v);
        setPref('lastWindow', v);
      }
      update();
      closeSoon();
    },
    async 'pick-area'() {
      setOpen(null, true);
      await lumos.invoke('hide_window', { label: 'menu' });
      const rect = await lumos.invoke('pick_area');
      if (rect) setPref('source', { kind: 'area', rect });
      update();
      await lumos.invoke('show_window', { label: 'menu' });
    },
    async record() {
      err = null; starting = true;
      const go = $('.go');
      if (go) { go.disabled = true; go.querySelector('.go-label').textContent = 'Starting…'; }
      update();
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
    // Speaker notes: a floating card you can read while recording. It never appears in the video.
    async notes() { await lumos.invoke('hide_window', { label: 'menu' }); await lumos.invoke('show_window', { label: 'notes' }); },
    async library() { await lumos.invoke('hide_window', { label: 'menu' }); await lumos.invoke('show_window', { label: 'main' }); lumos.emit('show-tab', 'library'); },
    async settings() { await lumos.invoke('hide_window', { label: 'menu' }); await lumos.invoke('show_window', { label: 'main' }); lumos.emit('show-tab', 'settings'); },
    quit: () => lumos.invoke('quit'),
  };

  card.addEventListener('click', async (ev) => {
    const b = ev.target.closest('[data-act]');
    if (!b || b.disabled) return;
    try { await actions[b.dataset.act](b); } catch (e) { err = lumos.errText(e); update(); }
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
    try { await saveAudio({ [`audio.${key}`]: db }); } catch (e) { audio[key] = before; err = lumos.errText(e); update(); }
  });

  lumos.onEvent((ev) => {
    if (ev.type === 'recorder.stateChanged') { rec = ev.data; render(); }
    else if (ev.type === 'recorder.tick') { rec.elapsedUs = ev.data.elapsedUs; const t = $('.live .time'); if (t) t.textContent = lumos.fmtClock(rec.elapsedUs); }
  });

  // Each time the popover opens: reload devices, permissions and settings, play the entrance, and
  // preview the camera bubble.
  function enter() {
    card.classList.remove('enter');
    void card.offsetWidth;
    card.classList.add('enter');
  }
  window.addEventListener('focus', () => { open = null; err = null; enter(); refresh().then(syncBubble); });
  window.addEventListener('blur', async () => {
    const st = await lumos.call('recording.status').catch(() => rec);
    if (st.state === 'idle' || st.state === 'failed') lumos.invoke('hide_window', { label: 'bubble' }).catch(() => {});
  });

  new ResizeObserver(sizeWindow).observe(card);

  render();
  enter();
  refresh();
})();
