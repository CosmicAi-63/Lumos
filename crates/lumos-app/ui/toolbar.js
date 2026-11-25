// Recording toolbar: record, pause, resume, stop, discard, camera and mic toggles.
// State is a projection of engine events; every button is one engine tool.
(() => {
  const { lumos } = window;
  const pill = document.getElementById('pill');

  let st = { state: 'idle', elapsedUs: 0, pendingFinalize: [] };
  let camOn = JSON.parse(localStorage.getItem('camOn') ?? 'true');
  let micOn = JSON.parse(localStorage.getItem('micOn') ?? 'true');
  let saved = null;          // { id, name, path } after a recording is saved
  let note = null;           // { text, err } transient message
  let confirmingDiscard = false;
  let countdownEnd = 0;
  let timers = {};

  const grip = '<span class="grip" data-tauri-drag-region aria-hidden="true">⋮⋮</span>';
  const btn = (act, label, cls = '', extra = '') => `<button class="tb ${cls}" data-act="${act}" ${extra}>${label}</button>`;
  const sep = '<span class="sep"></span>';

  function flash(text, err, ms = 6000) {
    note = { text, err };
    clearTimeout(timers.note);
    timers.note = setTimeout(() => { note = null; render(); }, ms);
    render();
  }

  function render() {
    const s = st.state;
    const saving = (st.pendingFinalize || []).length > 0;
    let html = grip;
    if (s === 'idle' || s === 'failed') {
      html += btn('record', '<span class="dot"></span> Record', 'rec');
      html += sep;
      html += btn('cam', 'Camera', '', `aria-pressed="${camOn}" title="Face bubble and camera track"`);
      html += btn('mic', 'Mic', '', `aria-pressed="${micOn}" title="Microphone"`);
      html += sep;
      html += btn('library', 'Library');
      html += btn('settings', 'Settings');
      html += btn('quit', '✕', '', 'title="Quit Lumos" aria-label="Quit Lumos"');
      if (saving) html += `<span class="status" data-tauri-drag-region>Saving…</span>`;
      else if (saved) html += `<span class="status saved" data-tauri-drag-region>✓ Saved</span>${btn('reveal', 'Show')}`;
      if (note) html += `<span class="status ${note.err ? 'err' : ''}" data-tauri-drag-region>${lumos.esc(note.text)}</span>`;
    } else if (s === 'preparing') {
      html += `<span class="status" data-tauri-drag-region>Getting ready…</span>` + btn('cancel', 'Cancel');
    } else if (s === 'countdown') {
      html += `<span class="count" id="count">…</span>` + btn('cancel', 'Cancel');
    } else if (s === 'recording' || s === 'paused') {
      const paused = s === 'paused';
      html += `<span class="dot ${paused ? '' : 'pulse'}" data-tauri-drag-region></span>`;
      html += `<span class="time" id="time" data-tauri-drag-region>${lumos.fmtClock(st.elapsedUs)}</span>`;
      if (paused) html += `<span class="status" data-tauri-drag-region>Paused</span>`;
      html += sep;
      if (confirmingDiscard) {
        html += `<span class="status" data-tauri-drag-region>Discard this recording?</span>` + btn('discard-yes', 'Discard', 'discard') + btn('discard-no', 'Keep');
      } else {
        html += btn(paused ? 'resume' : 'pause', paused ? 'Resume' : 'Pause');
        html += btn('stop', '■ Stop', 'stop');
        html += btn('discard', 'Discard', 'discard', 'title="Throw this recording away"');
      }
      if (note) html += `<span class="status ${note.err ? 'err' : ''}" data-tauri-drag-region>${lumos.esc(note.text)}</span>`;
    } else {
      html += `<span class="status" data-tauri-drag-region>${s === 'discarding' ? 'Discarding…' : 'Working…'}</span>`;
    }
    pill.innerHTML = html;
    if (s === 'countdown') tickCountdown();
  }

  function tickCountdown() {
    clearInterval(timers.count);
    const update = () => {
      const el = document.getElementById('count');
      if (!el) return clearInterval(timers.count);
      el.textContent = Math.max(1, Math.ceil((countdownEnd - Date.now()) / 1000));
    };
    update();
    timers.count = setInterval(update, 150);
  }

  async function syncBubble() {
    try { await lumos.invoke(camOn ? 'show_window' : 'hide_window', { label: 'bubble' }); } catch (_) {}
  }

  const actions = {
    async record() {
      saved = null; note = null;
      try {
        const secs = await lumos.call('settings.get', { path: 'recording.countdownSeconds' });
        countdownEnd = Date.now() + (Number(secs) || 0) * 1000;
        st = await lumos.call('recording.start', { camera: { enabled: camOn }, mic: { enabled: micOn } });
        render();
      } catch (e) { flash(lumos.errText(e), true, 9000); }
    },
    pause: () => lumos.call('recording.pause'),
    resume: () => lumos.call('recording.resume'),
    async stop() { try { await lumos.call('recording.stop', { wait: false }); } catch (e) { flash(lumos.errText(e), true); } },
    cancel: () => lumos.call('recording.cancel', { confirm: true }),
    discard() { confirmingDiscard = true; render(); },
    'discard-no'() { confirmingDiscard = false; render(); },
    async 'discard-yes'() { confirmingDiscard = false; await lumos.call('recording.cancel', { confirm: true }); },
    cam() { camOn = !camOn; localStorage.setItem('camOn', camOn); syncBubble(); render(); },
    mic() { micOn = !micOn; localStorage.setItem('micOn', micOn); render(); },
    async library() { await lumos.invoke('show_window', { label: 'main' }); lumos.emit('show-tab', 'library'); },
    async settings() { await lumos.invoke('show_window', { label: 'main' }); lumos.emit('show-tab', 'settings'); },
    async reveal() { if (saved) await lumos.call('library.reveal', { id: saved.id }); },
    quit: () => lumos.invoke('quit'),
  };

  pill.addEventListener('click', async (ev) => {
    const b = ev.target.closest('[data-act]');
    if (!b) return;
    try { await actions[b.dataset.act](); } catch (e) { flash(lumos.errText(e), true); }
  });

  lumos.onEvent((ev) => {
    switch (ev.type) {
      case 'recorder.stateChanged':
        if (ev.data.state !== 'recording' && ev.data.state !== 'paused') confirmingDiscard = false;
        st = ev.data; render(); break;
      case 'recorder.tick': {
        st.elapsedUs = ev.data.elapsedUs;
        const t = document.getElementById('time');
        if (t) t.textContent = lumos.fmtClock(st.elapsedUs);
        break;
      }
      case 'recorder.warning': flash(ev.data.message || 'Warning', true, 8000); break;
      case 'recording.ready':
        saved = { id: ev.data.recordingId, name: lumos.basename(ev.data.outputPath), path: ev.data.outputPath };
        clearTimeout(timers.saved); timers.saved = setTimeout(() => { saved = null; render(); }, 15000);
        render(); break;
      case 'recording.failed': flash(`Couldn't save: ${(ev.data.error && ev.data.error.message) || 'unknown error'}`, true, 12000); break;
    }
  });

  // Keep the transparent window as small as the pill so it never blocks clicks beneath it.
  new ResizeObserver(() => {
    const r = pill.getBoundingClientRect();
    lumos.invoke('set_window_size', { label: 'toolbar', width: Math.ceil(r.width) + 16, height: Math.ceil(r.height) + 16 }).catch(() => {});
  }).observe(pill);

  render();
  syncBubble();
  lumos.call('recording.status').then((s) => { st = s; render(); }).catch((e) => flash(lumos.errText(e), true));
})();
