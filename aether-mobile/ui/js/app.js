// Aether mobile — bridges the Stitch design to the Tauri backend.
// Works with or without the native bridge (browser preview uses a mock).

const inTauri = typeof window.__TAURI__ !== 'undefined' && window.__TAURI__.core;
const invoke = inTauri
  ? (cmd, args) => window.__TAURI__.core.invoke(cmd, args)
  : async (cmd) => { if (cmd === 'get_status') return mockStatus; if (cmd === 'get_settings') return settings; return null; };
const listen = inTauri
  ? (evt, cb) => window.__TAURI__.event.listen(evt, (e) => cb(e.payload))
  : async () => {};

// ---------------- state ----------------
let settings = { protocol: 'masque', scan: 'balanced', obfuscation: 'balanced', server: '' };
let status = { phase: 'disconnected', detail: 'Disconnected', protocol: 'MASQUE / HTTP/3', socks: '127.0.0.1:1819' };
let mockStatus = { phase: 'disconnected', detail: 'Disconnected', protocol: 'MASQUE / HTTP/3', socks: '127.0.0.1:1819', latency_ms: null };

const $ = (id) => document.getElementById(id);

const PROTOCOL_LABEL = { 'masque': 'MASQUE / HTTP/3', 'masque-h2': 'MASQUE / HTTP/2', 'wg': 'WireGuard' };

// ---------------- dock navigation ----------------
function setupDock() {
  const tabs = document.querySelectorAll('.tab');
  const pill = $('dockPill');
  function move(activeTab) {
    const i = [...tabs].indexOf(activeTab);
    pill.style.transform = `translateX(${i * 100}%)`;
  }
  tabs.forEach((t) =>
    t.addEventListener('click', () => {
      tabs.forEach((x) => x.classList.remove('active'));
      t.classList.add('active');
      document.querySelectorAll('.view').forEach((v) => v.classList.remove('active'));
      $('view-' + t.dataset.view).classList.add('active');
      move(t);
    })
  );
  requestAnimationFrame(() => move(document.querySelector('.tab.active')));
}

// ---------------- proxy view ----------------
function renderStatus() {
  const btn = $('connectBtn'), label = $('btnLabel'), dot = $('statusDot'),
    txt = $('statusText'), lat = $('valLatency'), up = $('valUptime'),
    exit = $('valExit'), proto = $('valProtocol'), sock = $('valSocks'), hint = $('hintSocks');

  const p = status.phase;
  const running = p === 'connected' || p === 'connecting' || p === 'scanning' || p === 'provisioning';

  btn.classList.toggle('on', p === 'connected');
  btn.classList.toggle('busy', p === 'scanning' || p === 'connecting' || p === 'provisioning');
  btn.setAttribute('aria-pressed', p === 'connected' ? 'true' : 'false');
  label.textContent = p === 'connected' ? 'Disconnect' : (running ? 'Please wait…' : 'Connect');

  dot.className = 'dot' + (p === 'connected' ? ' green' : p === 'error' ? ' red' : running ? ' amber' : '');
  txt.className = 'status-text' + (p === 'connected' ? ' green' : p === 'error' ? ' red' : '');
  txt.textContent = status.detail || (p === 'connected' ? 'Connected' : 'Disconnected');

  proto.textContent = status.protocol || proto.textContent;
  sock.textContent = status.socks || '127.0.0.1:1819';
  hint.textContent = status.socks || '127.0.0.1:1819';

  lat.textContent = status.latency_ms != null ? status.latency_ms + 'ms' : '--';
  lat.className = status.latency_ms != null ? '' : 'dim';

  up.className = p === 'connected' ? '' : 'dim';
  if (!p.startsWith('conn') && p !== 'connected') up.textContent = '0s';

  if (status.exit_ip) {
    exit.textContent = `${status.colo || '?'} · ${status.loc || '?'} · ${status.exit_ip}`;
    exit.className = '';
  } else {
    exit.textContent = p === 'connected' ? 'fetching…' : '--';
    exit.className = 'dim';
  }
}

async function toggleConnect() {
  const p = status.phase;
  if (p === 'connected' || p === 'connecting' || p === 'scanning' || p === 'provisioning') {
    await invoke('disconnect');
    status.phase = 'disconnected'; status.detail = 'Disconnected'; status.latency_ms = null;
    renderStatus();
  } else {
    await invoke('save_settings', { settings });
    try {
      await invoke('connect', { settings });
      status.phase = 'provisioning'; status.detail = 'Preparing device identity…';
      renderStatus();
    } catch (e) {
      status.phase = 'error'; status.detail = String(e); renderStatus();
    }
  }
}

function setupDetailsToggle() {
  const head = $('detailsToggle'), rows = $('detailsRows'), chev = $('detailsChev');
  head.addEventListener('click', () => {
    const hidden = rows.classList.toggle('hidden');
    head.setAttribute('aria-expanded', (!hidden).toString());
    chev.classList.toggle('closed', hidden);
  });
}

// uptime ticker
setInterval(() => {
  if (status.phase === 'connected' && status.detail && status.detail.includes('·')) {
    $('valUptime').textContent = status.detail.split('·')[1].trim();
  }
}, 1000);

// ---------------- settings view ----------------
function setupProtocolMenu() {
  const btn = $('protocolBtn'), menu = $('protocolMenu'), caret = btn.querySelector('.caret');
  btn.addEventListener('click', (e) => { e.stopPropagation(); const open = !menu.hidden; menu.hidden = open; caret.classList.toggle('up', !open); });
  document.addEventListener('click', () => { menu.hidden = true; caret.classList.remove('up'); });
  menu.querySelectorAll('.opt').forEach((opt) => {
    opt.addEventListener('click', () => {
      menu.querySelectorAll('.opt').forEach((o) => o.classList.remove('sel'));
      opt.classList.add('sel');
      $('protocolLabel').textContent = opt.dataset.label;
      $('protocolDesc').textContent = opt.dataset.desc;
      settings.protocol = opt.dataset.val;
      status.protocol = PROTOCOL_LABEL[settings.protocol];
      menu.hidden = true; caret.classList.remove('up');
      persist();
    });
  });
}

function setupSeg(id, key, hintId) {
  const buttons = document.querySelectorAll(`.seg[data-group="${id}"] .seg-btn`);
  buttons.forEach((b) =>
    b.addEventListener('click', () => {
      buttons.forEach((x) => x.classList.remove('active'));
      b.classList.add('active');
      settings[id] = b.dataset.val;
      if (hintId) $(hintId).textContent = b.dataset.hint;
      persist();
    })
  );
}

function setupServer() {
  const input = $('serverInput');
  input.addEventListener('input', () => { settings.server = input.value.trim(); persist(); });
}

function setupRestore() {
  $('restoreBtn').addEventListener('click', () => {
    settings = { protocol: 'masque', scan: 'balanced', obfuscation: 'balanced', server: '' };
    applySettingsToForm();
    persist();
  });
}

function persist() {
  invoke('save_settings', { settings });
  flashSaved();
}
let saveT;
function flashSaved() {
  const s = $('savedFlash'); if (!s) return;
  s.hidden = false; clearTimeout(saveT); saveT = setTimeout(() => (s.hidden = true), 1500);
}

function applySettingsToForm() {
  // protocol
  document.querySelectorAll('.menu .opt').forEach((o) => o.classList.toggle('sel', o.dataset.val === settings.protocol));
  const sel = document.querySelector(`.menu .opt[data-val="${settings.protocol}"]`);
  if (sel) { $('protocolLabel').textContent = sel.dataset.label; $('protocolDesc').textContent = sel.dataset.desc; }
  // scan + obfuscation segments
  for (const grp of ['scan', 'obfuscation']) {
    document.querySelectorAll(`.seg[data-group="${grp}"] .seg-btn`).forEach((b) => {
      const on = b.dataset.val === settings[grp];
      b.classList.toggle('active', on);
      if (on) $(grp === 'scan' ? 'scanHint' : 'obfHint').textContent = b.dataset.hint;
    });
  }
  $('serverInput').value = settings.server || '';
}

// ---------------- console view ----------------
let filter = 'all';
const evWords = ['valid', 'connected', 'select', 'chosen', 'ready', 'bound', 'failover', 'keepalive', 'confirmed', 'listening', 'exposing', 'selected', '+'];
function classify(line) {
  const lvl = (line.level || '').toLowerCase();
  const msg = line.message || '';
  if (lvl === 'error' || msg.includes('[-]')) return 'err';
  if (lvl === 'warn') return 'info';
  if (msg.includes('[+]') || evWords.some((w) => msg.toLowerCase().includes(w))) return 'ev';
  if (lvl === 'debug') return 'dbg';
  return 'info';
}
function fmtTime() {
  const d = new Date();
  return [d.getHours(), d.getMinutes(), d.getSeconds()].map((n) => String(n).padStart(2, '0')).join(':');
}
function addLog(line) {
  const box = $('logbox');
  if (box.querySelector('.lg-empty')) box.innerHTML = '';
  const kind = classify(line);
  const el = document.createElement('div');
  el.className = 'lg ' + (kind === 'ev' ? 'ev' : kind === 'err' ? 'err' : kind === 'dbg' ? 'dbg' : '');
  el.dataset.type = kind === 'err' ? 'errors' : kind === 'ev' ? 'events' : 'info';
  el.innerHTML = `<span class="t">${fmtTime()}</span><span class="d"></span><span class="m"></span>`;
  el.querySelector('.m').textContent = line.message;
  box.appendChild(el);
  while (box.children.length > 500) box.removeChild(box.firstChild);
  applyFilterOne(el);
  box.scrollTop = box.scrollHeight;
}
function applyFilterOne(el) {
  const show = filter === 'all' || (filter === 'events' ? el.dataset.type === 'events' : el.dataset.type === 'errors');
  el.style.display = show ? '' : 'none';
}
function applyFilter() {
  document.querySelectorAll('.lg').forEach(applyFilterOne);
}
function setupConsole() {
  document.querySelectorAll('.chip').forEach((c) =>
    c.addEventListener('click', () => {
      document.querySelectorAll('.chip').forEach((x) => x.classList.remove('active'));
      c.classList.add('active'); filter = c.dataset.filter; applyFilter();
    })
  );
  $('clearLog').addEventListener('click', () => {
    $('logbox').innerHTML = '<div class="lg-empty">Listening for active socket events…</div>';
  });
}

// ---------------- boot ----------------
(async function init() {
  setupDock(); setupDetailsToggle(); setupProtocolMenu();
  setupSeg('scan', 'scan', 'scanHint'); setupSeg('obfuscation', 'obfuscation', 'obfHint');
  setupServer(); setupRestore(); setupConsole();

  try { const s = await invoke('get_settings'); if (s) settings = { ...settings, ...s }; } catch (_) {}
  applySettingsToForm();
  try { const st = await invoke('get_status'); if (st) { status = st; renderStatus(); } } catch (_) {}

  await listen('aether-status', (st) => { status = st; renderStatus(); });
  await listen('aether-log', (line) => addLog(line));
  renderStatus();

  $('connectBtn').addEventListener('click', toggleConnect);
})();
