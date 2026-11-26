// Recording toolbar: appears only while a recording is being prepared, running or just saved.
// State is a projection of engine events; every button is one engine tool. Starting a recording
// happens in the menu-bar popover (menu.js).
(() => {
  const { lumos } = window;
  const pill = document.getElementById('pill');

  let st = { state: 'idle', elapsedUs: 0, pendingFinalize: [] };
  let saved = null;          // { id, name, path } after a recording is saved
  let note = null;           // { text, err } transient message
  let confirmingDiscard = false;
  let countdownEnd = 0;
  let timers = {};
  let shown = { toolbar: null, bubble: null };

  const grip = '<span class="grip" data-tauri-drag-region aria-hidden="true">⋮⋮</span>';
  const btn = (act, label, cls = '', extra = '') => `<button class="tb ${cls}" data-act="${act}" ${extra}>${label}</button>`;
  const sep = '<span class="sep"></span>';
  const camOn = () => { try { return JSON.parse(localStorage.getItem('camOn') ?? 'true'); } catch (_) { return true; } };

  function flash(text, err, ms = 6000) {
    note = { text, err };
    clearTimeout(timers.note);
    timers.note = setTimeout(() => { note = null; render(); }, ms);
    render();
  }

  // Re-read the recorder. Saving is a background job, so its end is not a recorder state change.
  async function refreshStatus() {
    try { st = await lumos.call('recording.status'); } catch (_) { /* keep the last state */ }
    render();
  }

  function setVisible(label, on) {
    if (shown[label] === on) return;
    shown[label] = on;
    lumos.invoke(on ? 'show_window' : 'hide_window', { label }).catch(() => { shown[label] = null; });
  }

  function render() {
    const s = st.state;
    const saving = (st.pendingFinalize || []).length > 0;
    const idle = s === 'idle' || s === 'failed';
    let html = grip;
    if (idle) {
      if (saving) html += `<span class="status" data-tauri-drag-region><span class="spin" aria-hidden="true"></span>Saving…</span>`;
      else if (saved) html += `<span class="status saved" data-tauri-drag-region>✓ Saved</span>${btn('reveal', 'Show')}${btn('library', 'Library')}${btn('dismiss', '✕', '', 'aria-label="Dismiss"')}`;
      if (note) html += `<span class="status ${note.err ? 'err' : ''}" data-tauri-drag-region>${lumos.esc(note.text)}</span>${btn('dismiss', '✕', '', 'aria-label="Dismiss"')}`;
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

    // The toolbar and face bubble exist only while they have something to show.
    const busy = !idle || saving || !!saved || !!note;
    setVisible('toolbar', busy);
    setVisible('bubble', !idle && camOn());
    // Keep checking while a save runs, in case an event was missed.
    clearTimeout(timers.poll);
    if (saving) timers.poll = setTimeout(refreshStatus, 2000);
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

  const actions = {
    pause: () => lumos.call('recording.pause'),
    resume: () => lumos.call('recording.resume'),
    async stop() { try { await lumos.call('recording.stop', { wait: false }); } catch (e) { flash(lumos.errText(e), true); } },
    cancel: () => lumos.call('recording.cancel', { confirm: true }),
    discard() { confirmingDiscard = true; render(); },
    'discard-no'() { confirmingDiscard = false; render(); },
    async 'discard-yes'() { confirmingDiscard = false; await lumos.call('recording.cancel', { confirm: true }); },
    dismiss() { saved = null; note = null; render(); },
    async library() { await lumos.invoke('show_window', { label: 'main' }); lumos.emit('show-tab', 'library'); },
    async reveal() { if (saved) await lumos.call('library.reveal', { id: saved.id }); },
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
        if (ev.data.state === 'countdown') {
          lumos.call('settings.get', { path: 'recording.countdownSeconds' }).then((secs) => { countdownEnd = Date.now() + (Number(secs) || 0) * 1000; tickCountdown(); }).catch(() => {});
        }
        if (ev.data.state === 'preparing') { saved = null; note = null; }
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
        refreshStatus(); break;
      case 'recording.failed':
        flash(`Couldn't save: ${(ev.data.error && ev.data.error.message) || 'unknown error'}`, true, 12000);
        refreshStatus(); break;
    }
  });

  // Keep the transparent window as small as the pill so it never blocks clicks beneath it.
  new ResizeObserver(() => {
    const r = pill.getBoundingClientRect();
    lumos.invoke('set_window_size', { label: 'toolbar', width: Math.ceil(r.width) + 16, height: Math.ceil(r.height) + 16 }).catch(() => {});
  }).observe(pill);

  render();
  refreshStatus();
})();
