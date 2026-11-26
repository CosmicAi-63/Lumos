// Library and settings window. Each control is one engine tool (docs/04-tools.md).
(() => {
  const { lumos } = window;
  const { esc } = lumos;
  const view = document.getElementById('view');
  const search = document.getElementById('search');
  let tab = 'library';
  let presets = [];

  function toast(text, err = false, ms = 4000) {
    const t = document.getElementById('toast');
    t.textContent = text;
    t.className = 'toast' + (err ? ' err' : '');
    t.hidden = false;
    clearTimeout(toast.t);
    toast.t = setTimeout(() => { t.hidden = true; }, ms);
  }
  const fail = (e) => toast(lumos.errText(e), true, 7000);

  function setTab(name) {
    tab = name;
    document.querySelectorAll('.tab').forEach((b) => b.setAttribute('aria-selected', String(b.dataset.tab === name)));
    search.hidden = name !== 'library';
    document.getElementById('import').hidden = name !== 'library';
    render();
  }

  // ---- library -------------------------------------------------------------

  async function renderLibrary() {
    const query = search.value.trim();
    const [list, health] = await Promise.all([
      lumos.call('library.list', { limit: 200, query: query || undefined }),
      lumos.call('system.health').catch(() => null),
    ]);
    if (!presets.length) presets = await lumos.call('export.presets').catch(() => []);
    let html = '';
    const pending = health && health.problems.find((p) => p.code === 'RECOVERY_PENDING');
    if (pending) {
      html += `<div class="notice"><span>${esc(pending.message)}: a recording was interrupted.</span><button class="btn small" data-act="recover">Recover</button></div>`;
    }
    if (!list.items.length) {
      html += query
        ? `<div class="empty"><b>No matches</b>Nothing is titled “${esc(query)}”.</div>`
        : `<div class="empty"><b>No recordings yet</b>Press Record in the toolbar, or import an existing video.</div>`;
    } else {
      html += '<div class="grid">' + list.items.map(card).join('') + '</div>';
    }
    view.innerHTML = html;
  }

  function card(r) {
    const thumb = r.thumbPath ? `style="background-image:url('${esc(lumos.fileUrl(r.thumbPath))}')"` : '';
    const date = new Date(r.createdAt).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' });
    const tags = r.tags.length ? `<div class="tags">${r.tags.map((t) => `<span class="tag">${esc(t)}</span>`).join('')}</div>` : '';
    const opts = presets.map((p) => `<option value="${esc(p.id)}">${esc(p.name)}</option>`).join('');
    const imported = r.origin === 'imported' ? ' · imported' : '';
    return `<article class="card" data-id="${esc(r.id)}">
      <button class="thumb" data-act="play" aria-label="Play ${esc(r.title)}" ${thumb}><span class="play">▶</span><span class="dur">${lumos.fmtClock(r.durationUs)}</span></button>
      <div class="meta"><div class="title" title="${esc(r.title)}">${esc(r.title)}</div>
        <div class="sub">${esc(date)} · ${lumos.fmtBytes(r.bytes)}${imported}</div>${tags}</div>
      <div class="actions">
        <button class="btn small" data-act="reveal">Show in Finder</button>
        <button class="btn small" data-act="rename">Rename</button>
        <button class="btn small" data-act="transcript">${r.hasTranscript ? 'Transcript' : 'Transcribe'}</button>
        <select class="btn small" data-act="export" aria-label="Export ${esc(r.title)}"><option value="">Export…</option>${opts}</select>
        <button class="btn small danger" data-act="delete">Delete</button>
      </div></article>`;
  }

  function openPlayer(r) {
    const m = document.getElementById('modal');
    m.className = 'modal';
    m.hidden = false;
    m.innerHTML = `<video controls autoplay src="${esc(lumos.fileUrl(r.outputPath))}"></video>`;
    m.onclick = (e) => { if (e.target === m) closePlayer(); };
  }
  function closePlayer() {
    const m = document.getElementById('modal');
    m.hidden = true; m.innerHTML = '';
  }

  const libraryActions = {
    async play(el, id) {
      const d = await lumos.call('library.get', { id });
      openPlayer(d.summary);
    },
    reveal: (el, id) => lumos.call('library.reveal', { id }),
    rename(el, id) {
      const t = el.closest('.card').querySelector('.title');
      const input = document.createElement('input');
      input.type = 'text'; input.value = t.textContent; input.className = 'search'; input.style.width = '100%';
      t.replaceWith(input);
      input.focus(); input.select();
      let done = false;
      const finish = async (save) => {
        if (done) return; done = true;
        const title = input.value.trim();
        if (save && title && title !== t.textContent) {
          try { await lumos.call('library.update', { id, title }); } catch (e) { fail(e); }
        }
        render();
      };
      input.addEventListener('keydown', (e) => { if (e.key === 'Enter') finish(true); if (e.key === 'Escape') finish(false); });
      input.addEventListener('blur', () => finish(true));
    },
    async transcript(el, id) {
      let t;
      try {
        t = await lumos.call('transcript.get', { id });
      } catch (e) {
        if (e.code !== 'NOT_FOUND') throw e;
        toast('Transcribing… this can take a while', false, 120000);
        await lumos.call('transcript.create', { id });
        return;
      }
      const m = document.getElementById('modal');
      m.className = 'modal';
      m.hidden = false;
      m.innerHTML = `<pre class="transcript">${esc(t.text.trim())}</pre>`;
      m.onclick = (e) => { if (e.target === m) closePlayer(); };
    },
    async export(el, id) {
      const preset = el.value; el.value = '';
      if (!preset) return;
      toast('Exporting…', false, 60000);
      await lumos.call('export.create', { id, preset });
    },
    async delete(el, id) {
      if (el.dataset.armed !== '1') {
        el.dataset.armed = '1'; el.textContent = 'Really delete?';
        setTimeout(() => { el.dataset.armed = ''; el.textContent = 'Delete'; }, 3500);
        return;
      }
      await lumos.call('library.delete', { id, confirm: true });
      toast('Moved to the Trash');
    },
    async recover() {
      toast('Recovering…', false, 60000);
      await lumos.call('library.recover', {});
    },
  };

  view.addEventListener('click', async (ev) => {
    const el = ev.target.closest('[data-act]');
    if (!el || el.tagName === 'SELECT' || tab !== 'library') return;
    const id = el.closest('.card')?.dataset.id;
    try { await libraryActions[el.dataset.act](el, id); } catch (e) { fail(e); }
  });
  view.addEventListener('change', async (ev) => {
    const el = ev.target.closest('select[data-act]');
    if (!el || tab !== 'library') return;
    try { await libraryActions[el.dataset.act](el, el.closest('.card').dataset.id); } catch (e) { fail(e); }
  });

  document.getElementById('import').addEventListener('click', async () => {
    try {
      const paths = await lumos.invoke('pick_files');
      if (!paths.length) return;
      toast(`Importing ${paths.length} file${paths.length > 1 ? 's' : ''}…`, false, 60000);
      await lumos.call('import.create', { paths });
    } catch (e) { fail(e); }
  });
  search.addEventListener('input', () => { clearTimeout(search.t); search.t = setTimeout(render, 200); });
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape') closePlayer(); });

  // ---- settings ------------------------------------------------------------

  const text = (path, label, v, hint) => `<label class="f"><span>${label}</span><input type="text" data-path="${path}" value="${esc(v)}">${hint ? `<span class="hint">${hint}</span>` : ''}</label>`;
  const num = (path, label, v, min, max, step = 1) => `<label class="f"><span>${label}</span><input type="number" data-path="${path}" data-type="number" value="${esc(v)}" min="${min}" max="${max}" step="${step}"></label>`;
  const check = (path, label, v, hint) => `<label class="check"><input type="checkbox" data-path="${path}" ${v ? 'checked' : ''}> <span>${label}</span></label>${hint ? `<div class="hint">${hint}</div>` : ''}`;
  const select = (path, label, v, options, type = '') => `<label class="f"><span>${label}</span><select data-path="${path}" ${type ? `data-type="${type}"` : ''}>${options.map(([val, name]) => `<option value="${esc(val)}" ${String(val) === String(v ?? '') ? 'selected' : ''}>${esc(name)}</option>`).join('')}</select></label>`;

  async function renderSettings() {
    const [s, health, cams, mics] = await Promise.all([
      lumos.call('settings.get'),
      lumos.call('system.health'),
      lumos.call('sources.cameras').catch(() => []),
      lumos.call('sources.microphones').catch(() => []),
    ]);
    const dev = (list) => [['', 'System default'], ...list.map((d) => [d.id, d.name])];
    const mcp = JSON.stringify({ mcpServers: { lumos: { command: 'lumos', args: ['mcp'] } } }, null, 2);
    const perms = Object.entries(health.permissions || {}).map(([k, v]) => {
      const fix = v === 'granted' || k === 'accessibility' ? '' : ` <button class="btn small" data-act="open-pane" data-kind="${esc(k)}">Open settings</button>`;
      return `<li>${esc(k)}: <b>${esc(v)}</b>${fix}</li>`;
    }).join('');
    const paneHint = Object.values(health.permissions || {}).some((v) => v !== 'granted')
      ? '<div class="hint">Open settings shows Lumos in Finder. Drag it into the list, or press +, and it turns on. Then restart Lumos.</div>' : '';
    const problems = health.problems.length
      ? health.problems.map((p) => `<li><b>${esc(p.code)}</b>: ${esc(p.message)}<div class="hint">Fix: ${esc(p.fix)}</div></li>`).join('')
      : '<li>No problems found.</li>';
    const free = health.saveFolder.freeMB != null ? `${Math.round(health.saveFolder.freeMB / 1024)} GB free` : 'free space unknown';
    view.innerHTML = `<div class="form">
      <fieldset><legend>Saving</legend>
        <label class="f"><span>Save recordings to</span><div class="row"><input type="text" data-path="storage.saveFolder" value="${esc(s.storage.saveFolder)}"><button class="btn" data-act="choose-folder">Choose…</button></div>
          <span class="hint">${free}${health.saveFolder.writable ? '' : ' · this folder is not writable'}</span></label>
        ${text('storage.fileNameTemplate', 'File name', s.storage.fileNameTemplate, 'Use {title} and {date:%Y-%m-%d %H.%M}.')}
        ${select('storage.keepSourceTracks', 'Keep source tracks', s.storage.keepSourceTracks, [['always', 'Always (lets you re-edit later)'], ['never', 'Never (MP4 only)']])}
      </fieldset>
      <fieldset><legend>Recording</legend>
        ${num('recording.countdownSeconds', 'Countdown (seconds)', s.recording.countdownSeconds, 0, 10)}
        ${select('capture.fps', 'Frame rate', s.capture.fps, [[24, '24 fps'], [30, '30 fps'], [60, '60 fps']], 'number')}
        ${check('capture.showCursor', 'Show the cursor', s.capture.showCursor)}
      </fieldset>
      <fieldset><legend>Camera</legend>
        ${check('camera.enabled', 'Record with the camera', s.camera.enabled)}
        ${select('camera.deviceId', 'Camera', s.camera.deviceId, dev(cams), 'nullable')}
        ${select('camera.shape', 'Bubble shape', s.camera.shape, [['circle', 'Circle'], ['roundedRect', 'Rounded'], ['rect', 'Rectangle']])}
        ${select('camera.size', 'Bubble size', s.camera.size, [['s', 'Small'], ['m', 'Medium'], ['l', 'Large']])}
        ${check('camera.mirror', 'Mirror the preview', s.camera.mirror)}
      </fieldset>
      <fieldset><legend>Audio</legend>
        ${check('audio.micEnabled', 'Record the microphone', s.audio.micEnabled)}
        ${select('audio.micDeviceId', 'Microphone', s.audio.micDeviceId, dev(mics), 'nullable')}
        ${check('audio.systemAudio', 'Record system audio', s.audio.systemAudio, 'Needs macOS 13 or later.')}
        ${num('audio.micGainDb', 'Microphone gain (dB)', s.audio.micGainDb, -20, 20, 0.5)}
        ${num('audio.syncOffsetMs', 'Audio sync adjustment (ms)', s.audio.syncOffsetMs, -500, 500, 10)}
        <div class="hint">If sound arrives before the picture, raise this (try 100). If it arrives after, lower it. Applies to new recordings.</div>
      </fieldset>
      <fieldset><legend>Transcripts</legend>
        <label class="f"><span>Whisper model file</span><input type="text" data-path="advanced.whisperModelPath" data-type="nullable" placeholder="/path/to/ggml-medium.bin" value="${esc(s.advanced.whisperModelPath ?? '')}">
          <span class="hint">A whisper.cpp <code>ggml-*.bin</code> model. Needed for the Transcribe button. Transcription runs on this Mac.</span></label>
      </fieldset>
      <fieldset><legend>AI agents and automation</legend>
        ${check('automation.agentMayStartRecording', 'Let AI agents start recordings', s.automation.agentMayStartRecording, 'A visible indicator always shows while recording, and agents get a countdown of at least 3 seconds.')}
        ${check('automation.agentMayDelete', 'Let AI agents delete recordings', s.automation.agentMayDelete, 'Deleted recordings go to the Trash.')}
        ${check('automation.cliIsTrusted', 'Treat the command line as you', s.automation.cliIsTrusted, 'Off: a program running <code>lumos</code> gets the same limits as an AI agent.')}
        <div><div class="hint" style="margin-bottom:6px">Add this to Claude, Cursor or any MCP client:</div><pre class="code" id="mcp">${esc(mcp)}</pre>
        <button class="btn small" data-act="copy-mcp" style="margin-top:6px">Copy</button></div>
      </fieldset>
      <fieldset><legend>Status</legend>
        <div class="hint">Capture engine: ${esc(health.backend)} · ${esc(health.ffmpeg || 'ffmpeg not found')}</div>
        <ul class="health">${perms}</ul>${paneHint}
        <ul class="health">${problems}</ul>
      </fieldset></div>`;
  }

  view.addEventListener('change', async (ev) => {
    const el = ev.target.closest('[data-path]');
    if (!el || tab !== 'settings') return;
    let v = el.type === 'checkbox' ? el.checked : el.value;
    if (el.dataset.type === 'number' || el.type === 'number') v = Number(v);
    if (el.dataset.type === 'nullable' && v === '') v = null;
    try {
      await lumos.call('settings.set', { patch: { [el.dataset.path]: v } });
      toast('Saved');
    } catch (e) { fail(e); renderSettings(); }
  });
  view.addEventListener('click', async (ev) => {
    const el = ev.target.closest('[data-act]');
    if (!el || tab !== 'settings') return;
    try {
      if (el.dataset.act === 'choose-folder') {
        const dir = await lumos.invoke('pick_folder');
        if (!dir) return;
        await lumos.call('settings.set', { patch: { 'storage.saveFolder': dir } });
        toast('Saved'); renderSettings();
      } else if (el.dataset.act === 'open-pane') {
        await lumos.invoke('open_privacy_pane', { kind: el.dataset.kind });
      } else if (el.dataset.act === 'copy-mcp') {
        await navigator.clipboard.writeText(document.getElementById('mcp').textContent);
        toast('Copied');
      }
    } catch (e) { fail(e); }
  });

  // ---- wiring --------------------------------------------------------------

  let rendering = false;
  async function render() {
    if (rendering) return;
    rendering = true;
    try { await (tab === 'library' ? renderLibrary() : renderSettings()); }
    catch (e) { view.innerHTML = `<div class="notice err">${esc(lumos.errText(e))}</div>`; }
    finally { rendering = false; }
  }

  document.querySelectorAll('.tab').forEach((b) => b.addEventListener('click', () => setTab(b.dataset.tab)));
  lumos.on('show-tab', (name) => setTab(name === 'settings' ? 'settings' : 'library'));
  lumos.onEvent((ev) => {
    if (ev.type === 'job.finished' && ev.data.status === 'failed') {
      toast(`${ev.data.kind} failed: ${(ev.data.error && ev.data.error.message) || ''}`, true, 8000);
    }
    if (tab !== 'library') return;
    if (['recording.ready', 'library.deleted', 'library.imported', 'library.changed'].includes(ev.type)) render();
    if (ev.type === 'transcribe.finished') { toast('Transcript ready'); render(); }
    if (ev.type === 'export.finished') { toast(`Exported ${lumos.basename(ev.data.path)}`); render(); }
    if (ev.type === 'job.finished' && ev.data.kind === 'recover') { toast('Recovery finished'); render(); }
  });

  setTab('library');
})();
