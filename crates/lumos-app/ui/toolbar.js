// Recording toolbar: appears only while a recording is being prepared, running or just saved.
// State is a projection of engine events; every button is one engine tool. Starting a recording
// happens in the menu-bar popover (menu.js).
(() => {
  const { lumos } = window;
  const pill = document.getElementById('pill');

  let st = { state: 'idle', elapsedUs: 0, pendingFinalize: [] };
  let saved = null;          // { id, name, path } after a recording is saved
  let note = null;           // { text, err } transient message
  let confirming = null;     // 'discard' | 'restart' while asking to confirm
  let drawing = false;       // the drawing overlay is open
  let countdownEnd = 0;
  let timers = {};
  let shown = { toolbar: null, bubble: null };

  const btn = (act, label, cls = '', extra = '') => `<button class="tb ${cls}" data-act="${act}" ${extra}>${label}</button>`;
  const ibtn = (act, svg, label, cls = '') => btn(act, svg, cls, `aria-label="${label}" title="${label}"`);
  const sep = '<span class="sep" data-tauri-drag-region></span>';
  const svg = (body, fill = false) => `<svg width="20" height="20" viewBox="0 0 20 20" fill="${fill ? 'currentColor' : 'none'}" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
  const icon = {
    stop: '<svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true"><rect x="1" y="1" width="14" height="14" rx="3.5" fill="currentColor"/></svg>',
    pause: svg('<rect x="5" y="3.5" width="3" height="13" rx="1.2" stroke="none"/><rect x="12" y="3.5" width="3" height="13" rx="1.2" stroke="none"/>', true),
    play: svg('<path d="M6.5 4.2v11.6a.8.8 0 0 0 1.2.7l9-5.8a.8.8 0 0 0 0-1.4l-9-5.8a.8.8 0 0 0-1.2.7z" stroke="none"/>', true),
    restart: svg('<path d="M3.8 9.5a6.4 6.4 0 1 0 2-4.6"/><path d="M5.6 2.2v3.2h3.2"/>'),
    trash: svg('<path d="M3.5 5.5h13"/><path d="M8 5.5V4a1 1 0 0 1 1-1h2a1 1 0 0 1 1 1v1.5"/><path d="M5 5.5l.8 10.3a1.5 1.5 0 0 0 1.5 1.4h5.4a1.5 1.5 0 0 0 1.5-1.4L15 5.5"/><path d="M8.5 8.8v5M11.5 8.8v5"/>'),
    pen: svg('<path d="M11.8 3.6l2.6 2.6"/><path d="M4 14.4l.6-3.2L12.9 3a1.3 1.3 0 0 1 1.8 0l.8.8a1.3 1.3 0 0 1 0 1.8L7.2 13.8z"/><path d="M11 16.8h6"/>'),
    x: svg('<path d="M5.5 5.5l9 9M14.5 5.5l-9 9"/>'),
    folder: svg('<path d="M2.8 6V14.5a1.5 1.5 0 0 0 1.5 1.5h11.4a1.5 1.5 0 0 0 1.5-1.5V8a1.5 1.5 0 0 0-1.5-1.5H10L8.4 4.5H4.3A1.5 1.5 0 0 0 2.8 6z"/>'),
    library: svg('<rect x="3" y="3" width="6" height="6" rx="1.3"/><rect x="11" y="3" width="6" height="6" rx="1.3"/><rect x="3" y="11" width="6" height="6" rx="1.3"/><rect x="11" y="11" width="6" height="6" rx="1.3"/>'),
  };
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
    let html = '';
    const msg = () => note ? `<span class="status ${note.err ? 'err' : ''}" data-tauri-drag-region>${lumos.esc(note.text)}</span>` : '';
    if (idle) {
      if (saving) html += `<span class="spin" aria-hidden="true"></span><span class="status" data-tauri-drag-region>Saving…</span>`;
      else if (saved) html += `<span class="status saved" data-tauri-drag-region>✓ Saved</span>${ibtn('reveal', icon.folder, 'Show in Finder')}${ibtn('library', icon.library, 'Open library')}${ibtn('dismiss', icon.x, 'Dismiss')}`;
      if (note) html += msg() + (saved ? '' : ibtn('dismiss', icon.x, 'Dismiss'));
    } else if (s === 'preparing') {
      html += `<span class="spin" aria-hidden="true"></span><span class="status" data-tauri-drag-region>Getting ready…</span>` + ibtn('cancel', icon.x, 'Cancel');
    } else if (s === 'countdown') {
      html += `<span class="count" id="count" data-tauri-drag-region>…</span>` + ibtn('cancel', icon.x, 'Cancel');
    } else if (s === 'recording' || s === 'paused') {
      const paused = s === 'paused';
      html += ibtn('stop', icon.stop, 'Stop and save', 'stop');
      html += `<span class="time ${paused ? 'paused' : ''}" id="time" data-tauri-drag-region>${lumos.fmtClock(st.elapsedUs)}</span>`;
      if (confirming) {
        const yes = confirming === 'discard' ? 'Discard' : 'Restart';
        html += `<span class="status" data-tauri-drag-region>${yes} this recording?</span>`;
        html += btn('confirm-yes', yes, 'text danger') + btn('confirm-no', 'Keep', 'text');
      } else {
        html += paused ? ibtn('resume', icon.play, 'Resume') : ibtn('pause', icon.pause, 'Pause');
        html += ibtn('restart', icon.restart, 'Restart recording');
        html += ibtn('discard', icon.trash, 'Delete recording');
        // Window recordings capture only that window, so drawings would not show up in them.
        if (st.source?.source?.kind !== 'window') {
          html += sep;
          html += btn('draw', icon.pen, `draw${drawing ? ' on' : ''}`, `aria-pressed="${drawing}" aria-label="Draw on screen" title="${drawing ? 'Stop drawing and clear (Esc)' : 'Draw on screen'}"`);
        }
      }
      html += msg();
    } else {
      html += `<span class="spin" aria-hidden="true"></span><span class="status" data-tauri-drag-region>${s === 'discarding' ? 'Discarding…' : 'Working…'}</span>`;
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

  // Turning drawing off closes the overlay, which also clears every stroke.
  async function setDraw(on) {
    if (drawing === on) return;
    drawing = on;
    render();
    try { await lumos.invoke('set_draw_mode', { on }); } catch (e) { drawing = false; flash(lumos.errText(e), true); }
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
    draw: () => setDraw(!drawing),
    pause: () => lumos.call('recording.pause'),
    resume: () => lumos.call('recording.resume'),
    async stop() { try { await lumos.call('recording.stop', { wait: false }); } catch (e) { flash(lumos.errText(e), true); } },
    cancel: () => lumos.call('recording.cancel', { confirm: true }),
    discard() { confirming = 'discard'; render(); },
    restart() { confirming = 'restart'; render(); },
    'confirm-no'() { confirming = null; render(); },
    async 'confirm-yes'() {
      const what = confirming;
      confirming = null;
      await lumos.call('recording.cancel', { confirm: true });
      // Start again with the same source, camera and mic the menu last used.
      if (what === 'restart') {
        let params = {};
        try { params = JSON.parse(localStorage.getItem('lastStart') || '{}'); } catch (_) { /* use defaults */ }
        await lumos.call('recording.start', params);
      }
    },
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
        // Never leave the full-screen overlay up once there is no toolbar button to close it.
        if (ev.data.state !== 'recording' && ev.data.state !== 'paused') { confirming = null; setDraw(false); }
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

  lumos.on('draw-exit', () => setDraw(false));
  // After clicking a toolbar button the toolbar has focus, so Cmd+Z lands here: pass it to the overlay.
  addEventListener('keydown', (e) => {
    if (!drawing) return;
    if (e.metaKey && !e.shiftKey && e.key.toLowerCase() === 'z') { e.preventDefault(); lumos.emit('draw-undo'); }
    else if (e.key === 'Escape') setDraw(false);
  });

  // Keep the transparent window as small as the pill so it never blocks clicks beneath it.
  new ResizeObserver(() => {
    const r = pill.getBoundingClientRect();
    lumos.invoke('set_window_size', { label: 'toolbar', width: Math.ceil(r.width) + 16, height: Math.ceil(r.height) + 16 }).catch(() => {});
  }).observe(pill);

  render();
  refreshStatus();
})();
