// Aether mobile UI. Everything shown here comes from the in-process core:
// the engine's own status events, its log stream and the VpnService state.
// There is no mock mode - if the bridge is missing the UI says so.

const tauri = window.__TAURI__;
const invoke = (cmd, args) => tauri.core.invoke(cmd, args);
const listen = (evt, cb) => tauri.event.listen(evt, (e) => cb(e.payload));
const $ = (id) => document.getElementById(id);

const MTU_BY_PROTOCOL = { 'masque': '1280 bytes (QUIC)', 'masque-h2': '1500 bytes (TCP)' };
const TRANSPORT_BY_PROTOCOL = { 'masque': 'MASQUE over HTTP/3 QUIC', 'masque-h2': 'MASQUE over HTTP/2 TCP', 'wg': 'WireGuard over UDP' };

let settings = { protocol: 'masque', scan: 'balanced', obfuscation: 'firewall', server: '' };
let status = { phase: 'disconnected', detail: 'Disconnected', protocol: '', latency_ms: null, socks: '127.0.0.1:1819' };
let mode = 'proxy';
let vpn = { state: 'off', detail: 'not requested' };
let logs = [];
let filter = 'all';
let counters = { events: 0, errors: 0 };
let uptimeSecs = 0;

/* ------------------------------------------------------------------ tabs */
function showView(name) {
  document.querySelectorAll('.view').forEach((v) => v.classList.toggle('active', v.id === 'view-' + name));
  document.querySelectorAll('.dock .tab').forEach((t) => t.classList.toggle('active', t.dataset.view === name));
}

function setupDock() {
  document.querySelectorAll('.dock .tab').forEach((t) => t.addEventListener('click', () => showView(t.dataset.view)));
  document.querySelectorAll('[data-path]').forEach((a) => a.addEventListener('click', () => showView(a.dataset.path)));
  $('goto-settings').addEventListener('click', () => showView('settings'));
  $('route-card').addEventListener('click', () => showView('settings'));
  showView('proxy');
}

/* --------------------------------------------------------- connect dial */
function running(p) { return p === 'connected' || p === 'connecting' || p === 'scanning' || p === 'provisioning'; }

function renderStatus() {
  const p = status.phase;
  const busy = p === 'scanning' || p === 'connecting' || p === 'provisioning';
  const power = $('main-power-btn');

  power.classList.toggle('ok', p === 'connected');
  power.classList.toggle('busy', busy);
  power.classList.toggle('err', p === 'error');
  $('icon-power').classList.toggle('hidden', p === 'connected' || busy || p === 'error');
  $('icon-sync').classList.toggle('hidden', !busy);
  $('icon-stop').classList.toggle('hidden', p !== 'connected');
  $('icon-error').classList.toggle('hidden', p !== 'error');

  $('progress-arc').classList.toggle('ok', p === 'connected');
  $('progress-arc').style.strokeDashoffset = p === 'connected' ? '0' : busy ? '289' : '578';
  $('glow-aura').classList.toggle('ok', p === 'connected');

  const dot = $('live-indicator-dot');
  dot.className = 'live-dot' + (p === 'connected' ? ' ok' : p === 'error' ? ' err' : busy ? ' busy' : '');
  $('status-primary-text').textContent = p === 'connected' ? 'Connected' : p === 'error' ? 'Connection failed' : busy ? 'Establishing…' : 'Disconnected';
  $('status-sub-text').textContent = status.detail || (p === 'connected' ? 'Tunnel verified end to end' : 'Tap the dial to establish a verified tunnel');

  const routeDot = $('route-dot');
  routeDot.className = 'route-dot' + (p === 'connected' ? ' ok' : p === 'error' ? ' err' : busy ? ' busy' : '');
  $('route-title').textContent = mode === 'vpn' ? 'System-wide VPN' : 'Local SOCKS5 proxy';
  $('route-sub').textContent = p === 'connected'
    ? (status.protocol || 'engine running')
    : (mode === 'vpn' ? 'VpnService · every app' : 'apps you point at 127.0.0.1:1819');

  const lat = status.latency_ms;
  $('stat-latency').textContent = lat != null ? String(lat) : '--';
  $('stat-latency').parentElement.classList.toggle('dim', lat == null);
  $('route-ping').textContent = lat != null ? lat + ' ms' : '-- ms';

  const exit = status.exit_ip ? (status.colo ? status.colo + ' · ' + status.exit_ip : status.exit_ip) : '--';
  $('stat-exit').textContent = exit;
  $('stat-exit-hint').textContent = status.loc || 'edge';

  $('socks-address').textContent = status.socks || '127.0.0.1:1819';
  $('protocol-name-display').textContent = status.protocol || '--';
  $('detail-transport').textContent = TRANSPORT_BY_PROTOCOL[settings.protocol] || '--';
  $('detail-mtu').textContent = MTU_BY_PROTOCOL[settings.protocol] || '--';
  $('detail-edge').textContent = status.exit_ip ? [status.colo, status.loc, status.exit_ip].filter(Boolean).join(' · ') : '--';
  $('detail-warp').textContent = status.warp || '--';
  $('active-engine-title').textContent = p === 'connected' ? (status.protocol || 'Engine running') : p === 'error' ? 'Engine stopped' : 'No active engine';
  $('active-engine-sub').textContent = p === 'connected' ? 'Serving ' + (status.socks || '127.0.0.1:1819') : 'Protocol and obfuscation are chosen in Settings';

  const badge = $('details-badge');
  badge.textContent = p;
  badge.className = 'pill' + (p === 'connected' ? ' ok' : p === 'error' ? ' err' : busy ? ' busy' : '');
  $('term-engine').textContent = mode === 'vpn' ? 'vpn · ' + vpn.state : 'proxy';
  $('header-mode-chip').textContent = mode === 'vpn' ? 'VPN' : 'PROXY';
  $('header-mode-chip').classList.toggle('vpn', mode === 'vpn');
  $('mode-hint').textContent = mode === 'vpn'
    ? 'System-wide · the VpnService captures every app'
    : 'Proxy only · nothing else is redirected';
  $('detail-mode').textContent = mode === 'vpn' ? 'VPN (VpnService)' : 'Proxy (SOCKS5)';
  $('detail-vpn').textContent = vpn.state + (vpn.detail ? ' · ' + vpn.detail : '');
}

function renderUptime() {
  const text = status.phase === 'connected' ? fmtUptime(uptimeSecs) : '00m 00s';
  $('card-uptime').textContent = text;
  $('stat-uptime').textContent = text;
}

function fmtUptime(secs) {
  const h = Math.floor(secs / 3600), m = Math.floor((secs % 3600) / 60), s = secs % 60;
  const pad = (n) => String(n).padStart(2, '0');
  return h > 0 ? `${pad(h)}h ${pad(m)}m ${pad(s)}s` : `${pad(m)}m ${pad(s)}s`;
}

async function refreshVpnState() {
  try {
    const st = await invoke('vpn_state');
    if (st) { vpn = st; renderStatus(); }
  } catch (_) {}
}

async function toggleConnect() {
  if (running(status.phase)) {
    await invoke('disconnect');
    try { await invoke('vpn_stop'); } catch (_) {}
    status.phase = 'disconnected'; status.detail = 'Disconnected';
    status.latency_ms = null; status.exit_ip = null; status.colo = null; status.loc = null; status.warp = null;
    uptimeSecs = 0;
    await refreshVpnState();
    renderStatus(); renderUptime();
    return;
  }

  $('status-primary-text').textContent = 'Establishing…';
  $('status-sub-text').textContent = 'Saving settings';
  try {
    await invoke('save_settings', { settings });
  } catch (e) {
    status.phase = 'error'; status.detail = String(e); return renderStatus();
  }

  if (mode === 'vpn') {
    // The tun comes up first so no app traffic escapes while the tunnel is
    // negotiated; the engine's own sockets are excluded from the VpnService.
    $('status-sub-text').textContent = 'Requesting VPN permission…';
    try {
      await invoke('vpn_start');
    } catch (e) {
      status.phase = 'error'; status.detail = 'VPN: ' + String(e); renderStatus(); return;
    }
    await refreshVpnState();
    $('status-sub-text').textContent = 'VPN ready · starting the core engine';
  }

  try {
    await invoke('connect', { settings });
    status.phase = 'provisioning';
    status.detail = mode === 'vpn' ? 'VPN up · preparing device identity…' : 'Preparing device identity…';
    renderStatus();
  } catch (e) {
    status.phase = 'error'; status.detail = String(e); renderStatus();
  }
}

function setupDetails() {
  const head = $('details-toggle-btn');
  head.addEventListener('click', () => {
    const body = $('details-drawer-content');
    const hidden = body.classList.toggle('hidden');
    head.setAttribute('aria-expanded', String(!hidden));
    $('drawer-chevron').classList.toggle('open', !hidden);
  });
  $('copy-endpoint-btn').addEventListener('click', async () => {
    await copyText(status.socks || '127.0.0.1:1819', 'Endpoint copied');
  });
}

/* -------------------------------------------------------------- console */
const EVENT_WORDS = ['validated', 'listening', 'connected', 'selected', 'chosen', 'ready', 'exposing', 'confirmed'];

function classify(line) {
  const level = (line.level || '').toLowerCase();
  const msg = line.message || '';
  if (level === 'error' || msg.includes('[-]')) return 'err';
  if (msg.includes('[+]') || EVENT_WORDS.some((w) => msg.toLowerCase().includes(w))) return 'ev';
  if (level === 'debug') return 'dbg';
  return 'info';
}

function badgeFor(msg) {
  const m = msg.match(/\[(AUTO #?\d*|NET|HTTP|EDGE|SYS|DNS|WG|TCP|UDP|QUIC|H2|MIM|GOOL|TOR)\]/i);
  if (m) return m[1].toUpperCase();
  if (msg.includes('[-]')) return 'ERR';
  if (msg.includes('[!]')) return 'WARN';
  return 'CORE';
}

function stamp() {
  const d = new Date();
  const pad = (n, w = 2) => String(n).padStart(w, '0');
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`;
}

function renderLogLine(line) {
  const box = $('logbox');
  const empty = box.querySelector('.lg-empty');
  if (empty) empty.remove();

  const kind = classify(line);
  const el = document.createElement('div');
  el.className = 'lg ' + kind;
  el.dataset.type = kind === 'err' ? 'errors' : kind === 'ev' ? 'events' : 'info';
  const t = document.createElement('span'); t.className = 't'; t.textContent = stamp();
  const b = document.createElement('span'); b.className = 'b'; b.textContent = badgeFor(line.message || '');
  const m = document.createElement('span'); m.className = 'm'; m.textContent = line.message || '';
  el.append(t, b, m);
  box.appendChild(el);

  if (kind === 'ev') counters.events += 1;
  if (kind === 'err') counters.errors += 1;
  while (box.children.length > 600) box.removeChild(box.firstChild);
  applyFilter(el);
  box.scrollTop = box.scrollHeight;
  renderStreamStatus();
}

function applyFilter(el) {
  const show = filter === 'all' || (filter === 'events' ? el.dataset.type === 'events' : el.dataset.type === 'errors');
  el.style.display = show ? '' : 'none';
}

function renderStreamStatus() {
  $('stream-status').textContent = `Listening · ${counters.events} events · ${counters.errors} errors`;
}

function setupConsole() {
  document.querySelectorAll('.chip').forEach((c) =>
    c.addEventListener('click', () => {
      document.querySelectorAll('.chip').forEach((x) => x.classList.remove('active'));
      c.classList.add('active');
      filter = c.dataset.filter;
      document.querySelectorAll('.lg').forEach(applyFilter);
    })
  );
  $('clearLog').addEventListener('click', () => {
    logs = [];
    counters = { events: 0, errors: 0 };
    $('logbox').innerHTML = '<div class="lg-empty">Listening for core events…</div>';
    renderStreamStatus();
  });
  $('copy-log').addEventListener('click', async () => {
    const text = logs.map((l) => l.message).join('\n');
    await copyText(text || 'no events yet', 'Stream copied');
  });
  renderStreamStatus();
}

async function copyText(text, toastText) {
  try {
    await tauri.clipboardManager.writeText(text);
    showToast(toastText);
  } catch (_) {
    try {
      await navigator.clipboard.writeText(text);
      showToast(toastText);
    } catch (_) {}
  }
}

let toastTimer;
function showToast(text) {
  const el = $('copy-toast');
  el.textContent = text;
  el.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove('show'), 1400);
}

/* ------------------------------------------------------------- settings */
function setMode(next) {
  mode = next === 'vpn' ? 'vpn' : 'proxy';
  document.querySelectorAll('#mode-group .seg-btn, #mode-group-settings .seg-btn').forEach((b) =>
    b.classList.toggle('active', b.dataset.mode === mode)
  );
  renderStatus();
}

function renderSettings() {
  document.querySelectorAll('#protocolMenu .opt').forEach((o) => o.classList.toggle('sel', o.dataset.val === settings.protocol));
  document.querySelectorAll('.seg-btn[data-group]').forEach((b) => {
    const group = b.dataset.group === 'obfuscation' ? 'obfuscation' : 'scan';
    b.classList.toggle('active', b.dataset.val === settings[group]);
  });
  $('scanHint').textContent = settings.scan;
  $('obfHint').textContent = settings.obfuscation;
  $('serverInput').value = settings.server || '';
}

let saveTimer;
async function persist() {
  try {
    await invoke('save_settings', { settings });
    $('savedFlash').classList.remove('hidden');
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => $('savedFlash').classList.add('hidden'), 1200);
  } catch (_) {}
}

function setupSettings() {
  document.querySelectorAll('#mode-group .seg-btn').forEach((b) => b.addEventListener('click', () => setMode(b.dataset.mode)));
  document.querySelectorAll('#mode-group-settings .seg-btn').forEach((b) => b.addEventListener('click', () => setMode(b.dataset.mode)));

  document.querySelectorAll('#protocolMenu .opt').forEach((o) =>
    o.addEventListener('click', () => {
      settings.protocol = o.dataset.val;
      renderSettings(); renderStatus(); persist();
    })
  );

  document.querySelectorAll('.seg-btn[data-group]').forEach((b) =>
    b.addEventListener('click', () => {
      const group = b.dataset.group === 'obfuscation' ? 'obfuscation' : 'scan';
      settings[group] = b.dataset.val;
      renderSettings(); persist();
    })
  );

  const server = $('serverInput');
  server.addEventListener('change', () => {
    settings.server = server.value.trim();
    persist();
  });

  $('restoreBtn').addEventListener('click', () => {
    settings = { protocol: 'masque', scan: 'balanced', obfuscation: 'firewall', server: '' };
    renderSettings(); renderStatus(); persist();
  });
}

/* ----------------------------------------------------------------- boot */
async function boot() {
  setupDock(); setupDetails(); setupConsole(); setupSettings();

  try {
    const loaded = await invoke('get_settings');
    if (loaded) settings = { ...settings, ...loaded };
  } catch (_) {}
  renderSettings();

  try {
    const current = await invoke('get_status');
    if (current) status = current;
  } catch (_) {}
  await refreshVpnState();
  renderStatus(); renderUptime();

  await listen('aether-status', (payload) => {
    const was = status.phase;
    status = payload;
    if (payload.phase === 'connected') {
      if (was !== 'connected') uptimeSecs = 0;
      $('ripple-wave').classList.remove('go');
      void $('ripple-wave').offsetWidth;
      $('ripple-wave').classList.add('go');
    }
    renderStatus();
  });

  await listen('aether-log', (line) => {
    logs.push(line);
    if (logs.length > 800) logs.shift();
    renderLogLine(line);
  });

  setInterval(() => {
    if (status.phase === 'connected') { uptimeSecs += 1; renderUptime(); }
  }, 1000);

  setInterval(refreshVpnState, 4000);
  $('main-power-btn').addEventListener('click', toggleConnect);
}

if (!tauri || !tauri.core) {
  document.body.innerHTML = '<main style="padding:24px;font-family:monospace;color:#ef4444">' +
    'Aether needs the native bridge (run the Android app, not a plain browser).</main>';
} else {
  boot();
}
