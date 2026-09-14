// Aether mobile UI — Calm Peach. Everything shown here comes from the
// in-process core: the engine's own status events, its log stream and the
// VpnService state. There is no mock mode — if the bridge is missing the UI
// says so. No CDN, no webfonts: the app has to work on a network that blocks
// them.

const tauri = window.__TAURI__;
const invoke = (cmd, args) => tauri.core.invoke(cmd, args);
const listen = (evt, cb) => tauri.event.listen(evt, (e) => cb(e.payload));
const $ = (id) => document.getElementById(id);

const TRANSPORT_BY_PROTOCOL = {
  masque: 'MASQUE over HTTP/3 QUIC',
  'masque-h2': 'MASQUE over HTTP/2 TCP',
  wg: 'WireGuard over UDP',
  gool: 'WireGuard inside WireGuard',
  mim: 'MASQUE inside MASQUE',
};
const PROTOCOL_LABEL = {
  masque: 'MASQUE / HTTP/3',
  'masque-h2': 'MASQUE / HTTP/2',
  wg: 'WireGuard',
  gool: 'WARP-in-WARP',
  mim: 'MASQUE-in-MASQUE',
};
const MODE_OPTIONS = [
  { val: 'masque', label: 'MASQUE', desc: 'Default · HTTP/3 QUIC datagram tunnel' },
  { val: 'masque-h2', label: 'MASQUE / HTTP/2', desc: 'TCP carrier · survives UDP filtering' },
  { val: 'wg', label: 'WireGuard', desc: 'High throughput · low latency UDP' },
  { val: 'gool', label: 'WARP-in-WARP', desc: 'Dual-hop routing for deeper masking' },
  { val: 'mim', label: 'MASQUE-in-MASQUE', desc: 'Cascaded HTTP/3 multi-hop relay' },
];
const SCAN_OPTIONS = [
  { val: 'turbo', label: 'Turbo', desc: 'Stop at the first candidate that answers' },
  { val: 'balanced', label: 'Balanced', desc: 'Collect a few, keep the fastest' },
  { val: 'thorough', label: 'Thorough', desc: 'Sweep whole ranges when all looks blocked' },
  { val: 'stealth', label: 'Stealth', desc: 'Few probes in flight, for strict networks' },
  { val: 'ironclad', label: 'Ironclad', desc: 'A real tunnel and a real request per gateway' },
];
const SCAN_LABEL = { turbo: 'Turbo', balanced: 'Balanced', thorough: 'Thorough', stealth: 'Stealth', ironclad: 'Ironclad' };

let settings = { protocol: 'masque', scan: 'balanced', obfuscation: 'firewall', server: '', inner_server: '', ip_family: 'v4', dns: '', route_block: '', route_direct: '', team: '', access_email: '', access_token: '', access_id: '', access_secret: '' };
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
  $('icon-stop').classList.toggle('hidden', p !== 'connected');
  $('icon-error').classList.toggle('hidden', p !== 'error');
  $('ring-spinner').classList.toggle('hidden', !busy);

  $('button-subtext').textContent = p === 'connected' ? 'Active & secure' : busy ? 'Routing traffic…' : p === 'error' ? 'Failed — try again' : 'Tap to connect';

  $('status-primary-text').textContent = p === 'connected' ? 'Connected' : p === 'error' ? 'Connection failed' : busy ? 'Establishing…' : 'Disconnected';
  $('status-sub-text').textContent = status.detail || (p === 'connected' ? 'Tunnel verified end to end' : 'Tap the dial to establish a verified tunnel');

  const lat = status.latency_ms;
  $('stat-latency').textContent = lat != null ? String(lat) : '--';
  $('stat-latency').parentElement.classList.toggle('dim', lat == null);

  const exit = status.exit_ip ? (status.colo ? status.colo + ' · ' + status.exit_ip : status.exit_ip) : '--';
  $('stat-exit').textContent = exit;
  $('stat-exit-hint').textContent = status.loc || 'edge';

  $('socks-address').textContent = status.socks || '127.0.0.1:1819';
  $('protocol-name-display').textContent = status.protocol || '--';
  $('detail-transport').textContent = TRANSPORT_BY_PROTOCOL[baseProtocol(settings.protocol)] || '--';
  $('detail-edge').textContent = status.exit_ip ? [status.colo, status.loc, status.exit_ip].filter(Boolean).join(' · ') : '--';
  $('detail-warp').textContent = status.warp || '--';

  const badge = $('details-badge');
  badge.textContent = p;
  badge.className = 'pill' + (p === 'connected' ? ' ok' : p === 'error' ? ' err' : busy ? ' busy' : '');
  $('header-mode-chip').textContent = mode === 'vpn' ? 'VPN' : 'PROXY';
  $('detail-mode').textContent = mode === 'vpn' ? 'VPN (VpnService)' : 'Proxy (SOCKS5)';
  $('detail-vpn').textContent = vpn.state + (vpn.detail ? ' · ' + vpn.detail : '');
}

/* A nested protocol is carried by one of the two base transports, so the
 * transport line in telemetry always names the carrier. */
function baseProtocol(p) {
  if (p === 'gool') return 'wg';
  if (p === 'mim') return 'masque';
  return p;
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
}

/* ------------------------------------------------------------- console */
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
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function renderLogLine(line) {
  const box = $('logbox');
  const empty = box.querySelector('.lg-empty');
  if (empty) empty.remove();

  const kind = classify(line);
  const el = document.createElement('div');
  el.className = 'lg t-' + kind;
  el.dataset.type = kind === 'err' ? 'errors' : kind === 'ev' ? 'events' : 'info';

  const head = document.createElement('div');
  head.className = 'lg-head';
  const badge = document.createElement('span'); badge.className = 'lg-badge'; badge.textContent = badgeFor(line.message || '');
  const time = document.createElement('span'); time.className = 'lg-time'; time.textContent = stamp();
  head.append(badge, time);

  const msg = document.createElement('span'); msg.className = 'lg-msg'; msg.textContent = line.message || '';
  el.append(head, msg);
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
  $('clear-log').addEventListener('click', () => {
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
  const el = $('toast');
  el.textContent = text;
  el.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.remove('show'), 1400);
}

/* ------------------------------------------------------------- sheets */
function openSheet(id) {
  $(id).classList.add('open');
}
function closeSheet(id) {
  $(id).classList.remove('open');
}

function buildSheet(containerId, options, current, onPick) {
  const box = $(containerId);
  box.innerHTML = '';
  options.forEach((opt) => {
    const btn = document.createElement('button');
    btn.className = 'opt' + (opt.val === current ? ' sel' : '');
    btn.type = 'button';
    btn.innerHTML = `<span><span class="opt-label">${opt.label}</span><span class="opt-desc">${opt.desc}</span></span>
      <span class="opt-check"><svg class="ic" viewBox="0 0 24 24"><path d="m5 13 4 4L19 7"/></svg></span>`;
    btn.addEventListener('click', () => { onPick(opt.val); closeSheet('mode-sheet'); closeSheet('scan-sheet'); });
    box.appendChild(btn);
  });
}

function setupSheets() {
  $('mode-row-btn').addEventListener('click', () => {
    buildSheet('mode-sheet-options', MODE_OPTIONS, settings.protocol, (val) => {
      settings.protocol = val;
      renderSettings(); persist();
    });
    openSheet('mode-sheet');
  });
  $('scan-row-btn').addEventListener('click', () => {
    const label = settings.ip_family === 'v6' ? 'IPv6' : settings.ip_family === 'both' ? 'Dual-stack' : 'IPv4';
    const opts = SCAN_OPTIONS.map((o) => ({ ...o, label: `${label} · ${o.label}` }));
    buildSheet('scan-sheet-options', opts, `${label} · ${SCAN_LABEL[settings.scan] || settings.scan}`, () => {
      // The sheet only re-words the scan; the IP family lives in Settings.
      renderSettings();
    });
    openSheet('scan-sheet');
  });
  $('close-mode-sheet').addEventListener('click', () => closeSheet('mode-sheet'));
  $('close-scan-sheet').addEventListener('click', () => closeSheet('scan-sheet'));
  $('mode-sheet').addEventListener('click', (e) => { if (e.target.id === 'mode-sheet') closeSheet('mode-sheet'); });
  $('scan-sheet').addEventListener('click', (e) => { if (e.target.id === 'scan-sheet') closeSheet('scan-sheet'); });
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
  document.querySelectorAll('#protocol-menu .opt').forEach((o) => o.classList.toggle('sel', o.dataset.val === settings.protocol));
  document.querySelectorAll('.seg-btn[data-group]').forEach((b) => {
    const group = b.dataset.group;
    b.classList.toggle('active', b.dataset.val === settings[group]);
  });
  $('server-input').value = settings.server || '';
  $('inner-server-input').value = settings.inner_server || '';
  $('dns-input').value = settings.dns || '';
  $('route-direct-input').value = settings.route_direct || '';
  $('route-block-input').value = settings.route_block || '';
  $('team-input').value = settings.team || '';
  $('access-token-input').value = settings.access_token || '';

  $('current-mode-label').textContent = PROTOCOL_LABEL[settings.protocol] || 'MASQUE';
  $('mode-row-value').textContent = PROTOCOL_LABEL[settings.protocol] || 'MASQUE';
  const label = settings.ip_family === 'v6' ? 'IPv6' : settings.ip_family === 'both' ? 'Dual' : 'IPv4';
  $('scan-row-value').textContent = `${label} · ${SCAN_LABEL[settings.scan] || settings.scan}`;
}

let saveTimer;
async function persist() {
  try {
    await invoke('save_settings', { settings });
    $('saved-flash').classList.remove('hidden');
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => $('saved-flash').classList.add('hidden'), 1200);
  } catch (_) {}
}

function readSettingsForm() {
  settings.server = $('server-input').value.trim();
  settings.inner_server = $('inner-server-input').value.trim();
  settings.dns = $('dns-input').value.trim();
  settings.route_direct = $('route-direct-input').value.trim();
  settings.route_block = $('route-block-input').value.trim();
  settings.team = $('team-input').value.trim();
  settings.access_token = $('access-token-input').value.trim();
}

function setupSettings() {
  document.querySelectorAll('#mode-group .seg-btn').forEach((b) => b.addEventListener('click', () => setMode(b.dataset.mode)));
  document.querySelectorAll('#mode-group-settings .seg-btn').forEach((b) => b.addEventListener('click', () => setMode(b.dataset.mode)));

  document.querySelectorAll('#protocol-menu .opt').forEach((o) =>
    o.addEventListener('click', () => {
      settings.protocol = o.dataset.val;
      renderSettings(); renderStatus(); persist();
    })
  );

  document.querySelectorAll('.seg-btn[data-group]').forEach((b) =>
    b.addEventListener('click', () => {
      const group = b.dataset.group;
      settings[group] = b.dataset.val;
      renderSettings(); persist();
    })
  );

  ['server-input', 'inner-server-input', 'dns-input', 'route-direct-input', 'route-block-input', 'team-input', 'access-token-input'].forEach((id) => {
    $(id).addEventListener('change', () => { readSettingsForm(); persist(); });
  });

  // Settings sections.
  document.querySelectorAll('#settings-tab-group .seg-btn').forEach((b) =>
    b.addEventListener('click', () => {
      document.querySelectorAll('#settings-tab-group .seg-btn').forEach((x) => x.classList.toggle('active', x === b));
      $('settings-basic').style.display = b.dataset.tab === 'basic' ? 'flex' : 'none';
      $('settings-advanced').style.display = b.dataset.tab === 'advanced' ? 'flex' : 'none';
    })
  );

  // Zero Trust enrolment. The core stores the token itself, so the UI only
  // hands over what it typed and reports the outcome.
  $('team-signin-btn').addEventListener('click', async () => {
    readSettingsForm();
    const team = settings.team.trim();
    if (!team) { showToast('Enter an organization slug'); return; }
    if (!settings.access_token.trim()) { showToast('Enter an access token'); return; }
    try {
      await invoke('team_sign_in', {
        team,
        accessToken: settings.access_token.trim(),
        accessId: settings.access_id.trim() || null,
        accessSecret: settings.access_secret.trim() || null,
      });
      showToast('Enrolled in ' + team);
    } catch (e) {
      showToast(String(e));
    }
  });

  $('restore-btn').addEventListener('click', () => {
    settings = { protocol: 'masque', scan: 'balanced', obfuscation: 'firewall', server: '', inner_server: '', ip_family: 'v4', dns: '', route_block: '', route_direct: '', team: '', access_email: '', access_token: '', access_id: '', access_secret: '' };
    renderSettings(); renderStatus(); persist();
  });
}

/* ----------------------------------------------------------------- boot */
async function boot() {
  setupDock(); setupDetails(); setupConsole(); setupSettings(); setupSheets();

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
    status = payload;
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
  document.body.innerHTML = '<main style="padding:24px;font-family:monospace;color:#ba1a1a">' +
    'Aether needs the native bridge (run the Android app, not a plain browser).</main>';
} else {
  boot();
}
