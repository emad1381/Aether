import { api } from './api.js';

document.addEventListener('DOMContentLoaded', async () => {
  // -------------------------------------------------------------------------
  // 1. STATE & CONFIGURATION
  // -------------------------------------------------------------------------
  let config = {
    protocol: 'masque',
    socks_port: 1819,
    http_port: 1820,
    scan_mode: 'balanced',
    ip_family: 'v4',
    noize: 'firewall',
    peer: '',
    dns: '1.1.1.1, 1.0.0.1',
    upstream: '',
    auto_system_proxy: true,
    auto_connect: true,
    bypass_list: '<local>;localhost;127.*;10.*;192.168.*;172.16.*;*.ir;bank.ir',
    route_direct: '',
    route_block: '',
    tunnel_mode: 'proxy',
    tor_enabled: false,
    tor_mode: 'carry',
    tor_bridges: false,
    tor_country: 'auto',
    tor_bind: '',
    tor_bridge_lines: '',
    psiphon_enabled: false,
    psiphon_mode: 'carry',
    psiphon_region: '',
    psiphon_shape: 'auto',
    psiphon_cdn_ips: '',
    psiphon_cdn_sni: '',
    psiphon_bin: '',
    psiphon_http: '',
    wiw_outer: '',
    wiw_inner: '',
    mim_outer: '',
    mim_inner: '',
    fragment: false,
    fragment_size: '16-32',
    fragment_delay: '2-10',
    launch_at_startup: false,
    start_minimized: false,
    close_to_tray: true,
    team: '',
    access_email: '',
    access_token: '',
    access_id: '',
    access_secret: ''
  };

  let currentState = 'idle'; // 'idle' | 'connecting' | 'connected' | 'error'
  let currentStatus = {
    state: 'disconnected',
    latency_ms: null,
    uptime_secs: 0,
    socks_endpoint: '127.0.0.1:1819',
    system_proxy_active: false,
    exit_ip: null,
    colo: null,
    error_message: null
  };

  let logCount = 0;
  let isCheckingUpdates = false;

  // Load Saved Config
  try {
    const saved = await api.getSavedConfig();
    if (saved) {
      config = { ...config, ...saved };
      if (!config.http_port) {
        config.http_port = 1820;
      }
    }
  } catch (_) {}

  // -------------------------------------------------------------------------
  // 2. WINDOW CONTROLS & DRAGGING (Frameless Window Title Bar)
  // -------------------------------------------------------------------------
  const titlebar = document.getElementById('window-titlebar');
  if (titlebar) {
    titlebar.addEventListener('mousedown', (e) => {
      if (e.target.closest('button')) return;
      if (e.buttons === 1) {
        api.startDragging();
      }
    });
  }

  const btnMin = document.getElementById('btn-win-min');
  const btnMax = document.getElementById('btn-win-max');
  const btnClose = document.getElementById('btn-win-close');

  if (btnMin) btnMin.addEventListener('click', () => api.minimizeWindow());
  if (btnMax) btnMax.addEventListener('click', () => api.maximizeWindow());
  if (btnClose) btnClose.addEventListener('click', () => api.closeWindow());

  // -------------------------------------------------------------------------
  // 3. NAVIGATION RAIL SWITCHING
  // -------------------------------------------------------------------------
  const navBtns = {
    'proxy-control': document.getElementById('nav-proxy'),
    'network-console': document.getElementById('nav-console'),
    'cdn-scanner': document.getElementById('nav-cdn'),
    'node-settings': document.getElementById('nav-settings')
  };

  const views = {
    'proxy-control': document.getElementById('view-proxy'),
    'network-console': document.getElementById('view-console'),
    'cdn-scanner': document.getElementById('view-cdn'),
    'node-settings': document.getElementById('view-settings')
  };

  function switchView(targetPath) {
    Object.keys(views).forEach((key) => {
      const view = views[key];
      const btn = navBtns[key];
      if (key === targetPath) {
        if (view) view.classList.remove('hidden');
        if (btn) {
          btn.className = 'nav-rail-btn w-full h-11 flex items-center justify-center transition-colors bg-[#18181c] border-l-2 border-primary-container text-primary-container';
        }
      } else {
        if (view) view.classList.add('hidden');
        if (btn) {
          btn.className = 'nav-rail-btn w-full h-11 flex items-center justify-center text-on-surface-variant border-l-2 border-transparent hover:bg-[#18181c] hover:text-on-surface transition-colors';
        }
      }
    });
  }

  Object.keys(navBtns).forEach((key) => {
    const btn = navBtns[key];
    if (btn) {
      btn.addEventListener('click', () => switchView(key));
    }
  });

  // -------------------------------------------------------------------------
  // 3b. CDN FRONTING SCANNER
  // -------------------------------------------------------------------------
  const cdnScanBtn = document.getElementById('btn-cdn-scan');
  const cdnApplyBtn = document.getElementById('btn-cdn-apply');
  const cdnStatus = document.getElementById('cdn-scan-status');
  const cdnCount = document.getElementById('cdn-scan-count');
  const cdnBarWrap = document.getElementById('cdn-scan-bar-wrap');
  const cdnBar = document.getElementById('cdn-scan-bar');
  const cdnEmpty = document.getElementById('cdn-scan-empty');
  const cdnTable = document.getElementById('cdn-scan-table');
  const cdnBody = document.getElementById('cdn-scan-body');
  let cdnReport = null;

  if (typeof api.onCdnScan === 'function') {
    api.onCdnScan((payload) => {
      const done = Number(payload.done) || 0;
      const total = Number(payload.total) || 0;
      if (cdnStatus) cdnStatus.textContent = total ? `testing ${done}/${total}` : 'resolving edges...';
      if (cdnBarWrap) cdnBarWrap.classList.remove('hidden');
      if (cdnBar) cdnBar.style.width = total ? `${Math.round((done / total) * 100)}%` : '0%';
    });
  }

  function renderCdnReport(report) {
    cdnReport = report;
    const edges = Array.isArray(report.edges) ? report.edges : [];
    if (cdnCount) cdnCount.textContent = `${report.reachable}/${report.tested} reachable`;
    if (cdnEmpty) cdnEmpty.classList.toggle('hidden', edges.length > 0);
    if (cdnTable) cdnTable.classList.toggle('hidden', edges.length === 0);
    if (cdnApplyBtn) cdnApplyBtn.classList.toggle('hidden', !report.ips);
    if (!cdnBody) return;
    cdnBody.innerHTML = '';
    edges.forEach((edge) => {
      const row = document.createElement('tr');
      row.className = 'border-b border-[#222227]';
      const handshake = edge.reachable
        ? `${edge.latency_ms} ms`
        : 'filtered';
      const tone = edge.reachable ? 'text-[#2dd4bf]' : 'text-outline';
      [edge.ip, edge.cdn, edge.sni].forEach((value) => {
        const cell = document.createElement('td');
        cell.className = 'py-1.5 pr-3 text-on-surface';
        cell.textContent = value;
        row.appendChild(cell);
      });
      const cell = document.createElement('td');
      cell.className = `py-1.5 pr-3 ${tone}`;
      cell.textContent = handshake;
      row.appendChild(cell);
      cdnBody.appendChild(row);
    });
  }

  if (cdnScanBtn) {
    cdnScanBtn.addEventListener('click', async () => {
      cdnScanBtn.disabled = true;
      if (cdnStatus) cdnStatus.textContent = 'resolving edges...';
      if (cdnBarWrap) cdnBarWrap.classList.remove('hidden');
      if (cdnBar) cdnBar.style.width = '0%';
      try {
        const report = await api.scanCdnEdges();
        renderCdnReport(report);
        if (cdnStatus) {
          cdnStatus.textContent = report.reachable
            ? `${report.reachable} edges answered`
            : 'no edge answered — the CDN networks look blocked here';
        }
      } catch (err) {
        if (cdnStatus) cdnStatus.textContent = `scan failed: ${err}`;
      } finally {
        cdnScanBtn.disabled = false;
        if (cdnBarWrap) cdnBarWrap.classList.add('hidden');
      }
    });
  }

  if (cdnApplyBtn) {
    cdnApplyBtn.addEventListener('click', async () => {
      if (!cdnReport || !cdnReport.ips) return;
      config.psiphon_cdn_ips = cdnReport.ips;
      config.psiphon_cdn_sni = cdnReport.sni || '';
      // Edges without the fronting shape are never consulted, so applying a
      // scan result also switches psiphon to CDN fronting.
      if (config.psiphon_shape !== 'cdn') {
        config.psiphon_shape = 'cdn';
      }
      const ipsInput = document.getElementById('cfg-psiphon-cdn-ips');
      if (ipsInput) ipsInput.value = config.psiphon_cdn_ips;
      const sniInput = document.getElementById('cfg-psiphon-cdn-sni');
      if (sniInput) sniInput.value = config.psiphon_cdn_sni;
      syncDropdownTick('panel-psiphon-shape', 'cdn');
      const cdnLabel = document.querySelector('#panel-psiphon-shape [data-value="cdn"] span');
      const labelPsShape = document.getElementById('label-psiphon-shape');
      if (labelPsShape && cdnLabel) labelPsShape.textContent = cdnLabel.textContent;
      try {
        await api.saveGuiConfig(config);
        if (cdnStatus) cdnStatus.textContent = 'saved — Psiphon shape set to CDN';
      } catch (err) {
        if (cdnStatus) cdnStatus.textContent = `could not save: ${err}`;
      }
    });
  }

  const ftEditSettings = document.getElementById('ft-edit-settings');
  if (ftEditSettings) {
    ftEditSettings.addEventListener('click', () => switchView('node-settings'));
  }

  // -------------------------------------------------------------------------
  // 4. PROXY DASHBOARD (156px True Circle Hardware Core)
  // -------------------------------------------------------------------------
  const mainDialBtn = document.getElementById('main-dial-btn');
  const ringGlow = document.getElementById('ring-glow');
  const rippleRing = document.getElementById('ripple-ring');
  const iconConn = document.getElementById('icon-connected');
  const iconSync = document.getElementById('icon-connecting');
  const iconIdle = document.getElementById('icon-idle');
  const iconErr = document.getElementById('icon-error');
  const connectingWrap = document.getElementById('connecting-spinner-wrap');
  const dialProgressPct = document.getElementById('dial-progress-pct');
  const statusPrimary = document.getElementById('status-primary');
  const statusSecondary = document.getElementById('status-secondary');
  const railStatusDot = document.getElementById('rail-status-dot');
  const railTooltipText = document.getElementById('rail-tooltip-text');

  const dtSocks = document.getElementById('dt-socks');
  const dtProto = document.getElementById('dt-proto');
  const dtExit = document.getElementById('dt-exit');
  const dtTorBadge = document.getElementById('dt-tor-badge');
  const dtPsiphonBadge = document.getElementById('dt-psiphon-badge');
  const dtCipher = document.getElementById('dt-cipher');
  const dtMtu = document.getElementById('dt-mtu');
  const ftActiveEngine = document.getElementById('ft-active-engine');

  // The engine announces the tor listener ("tor is ready") only once it
  // genuinely carries traffic, so the badge follows that event, not the tor
  // setting: a tor route that never came up must not claim to be tor.
  let torAddr = '';

  // Same rule for the psiphon carrier: the badge only appears once the engine
  // reports a live psiphon listener, and drops with the tunnel.
  let psiphonAddr = '';

  function getProtocolDisplayName(proto) {
    switch (proto) {
      case 'masque': return 'MASQUE HTTP/3 (QUIC)';
      case 'masque-h2': return 'MASQUE HTTP/2 (TCP)';
      case 'wg': return 'WireGuard';
      case 'gool': return 'WARP-in-WARP (gool)';
      case 'mim': return 'MASQUE-in-MASQUE';
      case 'tor': return 'Tor (Inside WARP)';
      case 'tor-reverse': return 'Tor (Tunnel through Tor)';
      case 'tor-only': return 'Tor Only';
      case 'psiphon': return 'Psiphon (Inside WARP)';
      case 'psiphon-reverse': return 'Psiphon (Tunnel through Psiphon)';
      case 'psiphon-only': return 'Psiphon Only';
      default: return proto.toUpperCase();
    }
  }

  function playRipple() {
    if (!rippleRing) return;
    rippleRing.classList.remove('animate-ripple-once');
    void rippleRing.offsetWidth;
    rippleRing.classList.add('animate-ripple-once');
  }

  function playErrorShake() {
    if (!mainDialBtn) return;
    mainDialBtn.classList.remove('animate-error-shake');
    void mainDialBtn.offsetWidth;
    mainDialBtn.classList.add('animate-error-shake');
  }

  // Auto Mode narrates its progress through status.protocol ("AUTO: Testing
  // MASQUE / HTTP/2 / firewall / v4", "AUTO: Switching to final port..."), so
  // the status line names the route the supervisor is actually working on.
  function engineLabel(payload) {
    return payload && typeof payload.protocol === 'string' ? payload.protocol.trim() : '';
  }

  // The exit IP used to wait for the next 25s poll. Fetch it as soon as the
  // tunnel reports Connected, and retry briefly in case the first probe raced
  // the SOCKS listener coming up.
  let traceInFlight = false;

  async function refreshTrace() {
    if (traceInFlight) return;
    traceInFlight = true;
    try {
      const trace = await api.fetchTraceInfo(config.socks_port);
      if (trace && trace.ip) {
        currentStatus.exit_ip = trace.ip;
        currentStatus.colo = trace.colo;
        currentStatus.loc = trace.loc;
        updateVisualState('connected', currentStatus);
      }
    } catch (_) {
      // The tunnel may still be settling; the next poll retries.
    } finally {
      traceInFlight = false;
    }
  }

  function refreshTraceSoon() {
    refreshTrace();
    setTimeout(() => { if (!currentStatus.exit_ip) refreshTrace(); }, 2000);
    setTimeout(() => { if (!currentStatus.exit_ip) refreshTrace(); }, 6000);
  }

  function updateVisualState(visualState, payload = {}) {
    const wasConnected = currentState === 'connected';
    currentState = visualState;
    // A tor listener only lives inside a running engine: anything that starts
    // a fresh process (or tears one down) drops the badge. A reconnect keeps
    // the same process, so it keeps the address.
    if (visualState !== 'connected' && visualState !== 'reconnecting') torAddr = '';
    if (visualState !== 'connected' && visualState !== 'reconnecting') psiphonAddr = '';
    if (!mainDialBtn || !ringGlow) return;

    // Reset base classes
    mainDialBtn.classList.remove('border-[#2dd4bf]', 'border-[#f2711c]', 'border-[#303036]', 'border-[#ef4444]');
    ringGlow.classList.remove('animate-pulse-fast', 'animate-breathe', 'bg-[#2dd4bf]/15', 'bg-primary-container/25', 'bg-[#ef4444]/20', 'bg-[#303036]/20');

    // Hide all icons
    [iconConn, iconSync, iconIdle, iconErr, connectingWrap].forEach((ic) => ic && ic.classList.add('hidden'));

    if (visualState === 'connected') {
      mainDialBtn.classList.add('border-[#2dd4bf]');
      ringGlow.classList.add('bg-[#2dd4bf]/15');
      if (connectingWrap) connectingWrap.classList.add('hidden');
      if (dialProgressPct) dialProgressPct.textContent = '';
      if (iconConn) iconConn.classList.remove('hidden');

      const loc = payload.colo ? `${payload.colo} · ${payload.loc || ''}` : (payload.loc || 'Connected Edge');
      const pingText = payload.latency_ms ? `${payload.latency_ms}ms` : 'active';
      if (statusPrimary) {
        statusPrimary.innerHTML = `Connected to ${loc} · <span class="text-secondary font-mono font-medium">${pingText}</span>`;
      }
      if (statusSecondary) {
        statusSecondary.textContent = engineLabel(payload) || getProtocolDisplayName(config.protocol);
      }
      if (railStatusDot) {
        railStatusDot.className = 'w-2 h-2 rounded-full bg-secondary';
      }
      if (railTooltipText) {
        railTooltipText.textContent = 'Tunnel Active (SOCKS5 + Proxy Ready)';
      }
      if (!wasConnected) refreshTraceSoon();
      playRipple();
    } else if (visualState === 'connecting') {
      mainDialBtn.classList.add('border-[#f2711c]');
      ringGlow.classList.add('bg-primary-container/25', 'animate-pulse-fast');
      if (connectingWrap) connectingWrap.classList.remove('hidden');
      if (iconSync) iconSync.classList.remove('hidden');
      const pctText = dialProgressPct && dialProgressPct.textContent ? dialProgressPct.textContent : '10%';
      if (statusPrimary) {
        statusPrimary.innerHTML = `Establishing tunnel · <span class="text-secondary font-mono font-medium">${pctText}</span>`;
      }
      if (statusSecondary) {
        statusSecondary.textContent = engineLabel(payload) || 'NEGOTIATING ROUTE · RESOLVING ANYCAST';
      }
      if (railStatusDot) {
        railStatusDot.className = 'w-2 h-2 rounded-full bg-[#f2711c] animate-ping';
      }
      if (railTooltipText) {
        railTooltipText.textContent = 'Negotiating Gateway Handshake';
      }
    } else if (visualState === 'error') {
      mainDialBtn.classList.add('border-[#ef4444]');
      ringGlow.classList.add('bg-[#ef4444]/20');
      if (connectingWrap) connectingWrap.classList.add('hidden');
      if (dialProgressPct) dialProgressPct.textContent = '';
      if (iconErr) iconErr.classList.remove('hidden');
      if (statusPrimary) {
        statusPrimary.innerHTML = '<span class="text-error font-medium">Handshake Dropped · Refused</span>';
      }
      if (statusSecondary) {
        statusSecondary.textContent = payload.error_message || 'ERR_SOCKS5_PROXY_REFUSED';
      }
      if (railStatusDot) {
        railStatusDot.className = 'w-2 h-2 rounded-full bg-error';
      }
      if (railTooltipText) {
        railTooltipText.textContent = 'Connection Error';
      }
      playErrorShake();
    } else {
      // Idle
      mainDialBtn.classList.add('border-[#303036]');
      ringGlow.classList.add('bg-[#303036]/20', 'animate-breathe');
      if (connectingWrap) connectingWrap.classList.add('hidden');
      if (dialProgressPct) dialProgressPct.textContent = '';
      if (iconIdle) iconIdle.classList.remove('hidden');
      if (statusPrimary) {
        statusPrimary.innerHTML = '<span class="text-outline">Engine Standby</span>';
      }
      if (statusSecondary) {
        statusSecondary.textContent = 'TUNNEL DEACTIVATED';
      }
      if (railStatusDot) {
        railStatusDot.className = 'w-2 h-2 rounded-full bg-outline';
      }
      if (railTooltipText) {
        railTooltipText.textContent = 'Aether Service Ready';
      }
    }

    // Update details drawer
    if (dtSocks) dtSocks.textContent = `127.0.0.1:${config.socks_port}`;
    if (dtProto) {
      if (config.psiphon_enabled && config.psiphon_mode === 'psiphon-only') {
        dtProto.textContent = getProtocolDisplayName('psiphon-only');
      } else if (config.tor_enabled && config.tor_mode === 'tor-only') {
        dtProto.textContent = getProtocolDisplayName('tor-only');
      } else {
        dtProto.textContent = getProtocolDisplayName(config.protocol);
      }
    }
    if (dtExit) dtExit.textContent = payload.exit_ip ? `${payload.exit_ip} (Anycast)` : '-- (Anycast)';
    if (dtTorBadge) {
      // Only a live tor listener earns the badge, and it drops with the tunnel.
      const show = visualState === 'connected' && torAddr !== '';
      dtTorBadge.classList.toggle('hidden', !show);
      if (show) dtTorBadge.title = `tor exit via ${torAddr}`;
    }
    if (dtPsiphonBadge) {
      // Same rule as the tor badge: only a live psiphon listener earns it.
      const show = visualState === 'connected' && psiphonAddr !== '';
      dtPsiphonBadge.classList.toggle('hidden', !show);
      if (show) dtPsiphonBadge.title = `psiphon via ${psiphonAddr}`;
    }
    if (dtCipher) dtCipher.textContent = config.protocol === 'wg' ? 'ChaCha20-Poly1305' : 'AES-128-GCM / ChaCha20';
    if (dtMtu) dtMtu.textContent = config.protocol === 'masque-h2' ? '1500 Bytes' : '1280 Bytes';
    if (ftActiveEngine) {
      if (config.psiphon_enabled && config.psiphon_mode === 'psiphon-only') {
        ftActiveEngine.textContent = 'PSIPHON ONLY · Standalone';
      } else if (config.tor_enabled && config.tor_mode === 'tor-only') {
        ftActiveEngine.textContent = 'TOR ONLY · Standalone';
      } else {
        ftActiveEngine.textContent = `${config.protocol.toUpperCase()} · ${config.scan_mode.toUpperCase()} Scan`;
      }
    }
  }

  // Dial Toggle Click
  if (mainDialBtn) {
    mainDialBtn.addEventListener('click', async () => {
      if (currentState === 'connected' || currentState === 'connecting') {
        try {
          await api.stopTunnel();
        } catch (err) {
          console.error('Stop error:', err);
        }
      } else {
        try {
          syncFormToConfig();
          await api.saveGuiConfig(config);
          if (dialProgressPct) dialProgressPct.textContent = '10%';
          updateVisualState('connecting');
          await api.startTunnel(config);
        } catch (err) {
          updateVisualState('error', { error_message: String(err) });
        }
      }
    });
  }

  // Progressive Disclosure Details Drawer
  const detailsToggleBtn = document.getElementById('details-toggle-btn');
  const detailsDrawer = document.getElementById('details-drawer');
  const detailsChevron = document.getElementById('details-chevron');

  if (detailsToggleBtn && detailsDrawer && detailsChevron) {
    detailsToggleBtn.addEventListener('click', () => {
      const isHidden = detailsDrawer.classList.toggle('hidden');
      detailsChevron.style.transform = isHidden ? 'rotate(0deg)' : 'rotate(180deg)';
    });
  }

  // -------------------------------------------------------------------------
  // 5. NETWORK CONSOLE
  // -------------------------------------------------------------------------
  const consoleLogFeed = document.getElementById('console-log-feed');
  const consoleCopyBtn = document.getElementById('console-copy-btn');
  const consoleCopyText = document.getElementById('console-copy-text');
  const consoleClearBtn = document.getElementById('console-clear-btn');
  const consoleEventCounter = document.getElementById('console-event-counter');

  function appendLogLine(entry) {
    if (!consoleLogFeed) return;

    logCount++;
    if (consoleEventCounter) {
      consoleEventCounter.textContent = `${logCount} event${logCount === 1 ? '' : 's'} recorded`;
    }

    const row = document.createElement('div');
    row.className = 'flex items-start gap-2 leading-relaxed hover:bg-white/[0.02] px-1 py-0.5 rounded transition-colors group';

    const timeSpan = document.createElement('span');
    timeSpan.className = 'text-outline select-none shrink-0 font-mono text-[11px] pt-0.5';
    timeSpan.textContent = entry.timestamp || '00:00:00';

    const tagSpan = document.createElement('span');
    const msg = entry.message || '';
    let tag = '[SYS]';
    let tagColor = 'text-outline';

    if (msg.includes('socks5') || msg.includes('http proxy') || msg.includes('listening')) {
      tag = '[NET]';
      tagColor = 'text-outline';
    } else if (msg.includes('selected') || msg.includes('gateway') || msg.includes('validated') || msg.includes('optimal')) {
      tag = '[EDGE]';
      tagColor = 'text-secondary';
    } else if (msg.includes('http/3') || msg.includes('quic') || msg.includes('http/2') || msg.includes('masque')) {
      tag = '[HTTP]';
      tagColor = 'text-outline';
    } else if (entry.level === 'WARN' || entry.level === 'ERROR' || msg.includes('[-]')) {
      tag = '[WARN]';
      tagColor = 'text-error';
    } else if (msg.includes('tor')) {
      tag = '[TOR]';
      tagColor = 'text-outline';
    }

    tagSpan.className = `font-semibold ${tagColor} shrink-0 select-none font-mono text-[11px]`;
    tagSpan.textContent = tag;

    const msgSpan = document.createElement('span');
    msgSpan.className = 'text-on-surface font-mono break-all text-[12px]';
    msgSpan.textContent = msg;

    row.appendChild(timeSpan);
    row.appendChild(tagSpan);
    row.appendChild(msgSpan);

    consoleLogFeed.appendChild(row);

    // Limit DOM lines to 1000
    if (consoleLogFeed.children.length > 1000) {
      consoleLogFeed.removeChild(consoleLogFeed.firstChild);
    }
    consoleLogFeed.scrollTop = consoleLogFeed.scrollHeight;
  }

  if (consoleCopyBtn) {
    consoleCopyBtn.addEventListener('click', () => {
      if (!consoleLogFeed) return;
      const text = Array.from(consoleLogFeed.querySelectorAll('div'))
        .map((r) => r.innerText.replace(/\n+/g, ' ').trim())
        .join('\n');
      navigator.clipboard.writeText(text).then(() => {
        if (consoleCopyText) {
          consoleCopyText.textContent = 'Copied';
          setTimeout(() => {
            consoleCopyText.textContent = 'Copy';
          }, 1500);
        }
      });
    });
  }

  if (consoleClearBtn) {
    consoleClearBtn.addEventListener('click', () => {
      if (!consoleLogFeed) return;
      consoleLogFeed.innerHTML = '';
      logCount = 0;
      if (consoleEventCounter) consoleEventCounter.textContent = '0 events recorded';
    });
  }

  // -------------------------------------------------------------------------
  // 6. SETTINGS VIEW (BASIC & ADVANCED TABS + CONTROLS)
  // -------------------------------------------------------------------------
  const tabBtnBasic = document.getElementById('tab-btn-basic');
  const tabBtnAdvanced = document.getElementById('tab-btn-advanced');
  const tabContentBasic = document.getElementById('tab-content-basic');
  const tabContentAdvanced = document.getElementById('tab-content-advanced');

  function setSettingsTab(isAdvanced) {
    const activeClasses = ['bg-[#24242a]', 'text-on-surface', 'border', 'border-[#353438]', 'font-medium'];
    const inactiveClasses = ['text-on-surface-variant', 'hover:text-on-surface'];

    if (isAdvanced) {
      if (tabContentBasic) tabContentBasic.classList.add('hidden');
      if (tabContentAdvanced) tabContentAdvanced.classList.remove('hidden');

      if (tabBtnAdvanced) {
        tabBtnAdvanced.classList.remove(...inactiveClasses);
        tabBtnAdvanced.classList.add(...activeClasses);
        tabBtnAdvanced.setAttribute('aria-selected', 'true');
      }
      if (tabBtnBasic) {
        tabBtnBasic.classList.remove(...activeClasses);
        tabBtnBasic.classList.add(...inactiveClasses);
        tabBtnBasic.setAttribute('aria-selected', 'false');
      }
    } else {
      if (tabContentAdvanced) tabContentAdvanced.classList.add('hidden');
      if (tabContentBasic) tabContentBasic.classList.remove('hidden');

      if (tabBtnBasic) {
        tabBtnBasic.classList.remove(...inactiveClasses);
        tabBtnBasic.classList.add(...activeClasses);
        tabBtnBasic.setAttribute('aria-selected', 'true');
      }
      if (tabBtnAdvanced) {
        tabBtnAdvanced.classList.remove(...activeClasses);
        tabBtnAdvanced.classList.add(...inactiveClasses);
        tabBtnAdvanced.setAttribute('aria-selected', 'false');
      }
    }
  }

  if (tabBtnBasic) tabBtnBasic.addEventListener('click', () => setSettingsTab(false));
  if (tabBtnAdvanced) tabBtnAdvanced.addEventListener('click', () => setSettingsTab(true));

  // Custom Dropdown Helper
  // Inline SVG tick: the Material Symbols font is no longer bundled, so the
  // old ligature span rendered the literal word "check" and the hard-coded
  // tick never left the first entry.
  const CHECK_ICON = '<svg class="dd-check w-3.5 h-3.5 text-[#f2711c]" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" viewBox="0 0 24 24"><polyline points="20 6 9 17 4 12"/></svg>';

  function selectDropdownOption(panel, opt) {
    panel.querySelectorAll('[data-value]').forEach((el) => {
      el.classList.remove('bg-[#2a2a32]');
      el.querySelectorAll('.dd-check, .material-symbols-outlined').forEach((icon) => icon.remove());
    });
    opt.classList.add('bg-[#2a2a32]');
    opt.insertAdjacentHTML('beforeend', CHECK_ICON);
  }

  // Keeps each dropdown tick on the value that is actually stored.
  function syncDropdownTick(panelId, value) {
    const panel = document.getElementById(panelId);
    if (!panel) return;
    const match = Array.from(panel.querySelectorAll('[data-value]'))
      .find((el) => el.getAttribute('data-value') === String(value));
    if (match) selectDropdownOption(panel, match);
  }

  function setupCustomDropdown(btnId, panelId, labelId, onChange) {
    const btn = document.getElementById(btnId);
    const panel = document.getElementById(panelId);
    const label = document.getElementById(labelId);
    if (!btn || !panel || !label) return;

    btn.addEventListener('click', (e) => {
      e.stopPropagation();
      const isOpen = panel.classList.contains('open');
      document.querySelectorAll('.dropdown-panel.open').forEach((p) => {
        if (p !== panel) p.classList.remove('open');
      });
      panel.classList.toggle('open', !isOpen);
      const chevron = btn.querySelector('svg');
      if (chevron) chevron.style.transform = isOpen ? 'rotate(0deg)' : 'rotate(180deg)';
    });

    panel.querySelectorAll('[data-value]').forEach((opt) => {
      opt.addEventListener('click', (e) => {
        e.stopPropagation();
        const val = opt.getAttribute('data-value');
        const text = opt.querySelector('span:first-child')?.textContent || opt.textContent;
        label.textContent = text.trim();

        selectDropdownOption(panel, opt);

        panel.classList.remove('open');
        const chevron = btn.querySelector('svg');
        if (chevron) chevron.style.transform = 'rotate(0deg)';

        if (onChange) onChange(val, text.trim());
      });
    });
  }

  document.addEventListener('click', () => {
    document.querySelectorAll('.dropdown-panel.open').forEach((p) => {
      p.classList.remove('open');
      const btn = p.previousElementSibling;
      const chevron = btn?.querySelector('svg');
      if (chevron) chevron.style.transform = 'rotate(0deg)';
    });
  });

  // 1. Protocol Dropdown
  setupCustomDropdown('btn-proto', 'panel-proto', 'label-proto', (val) => {
    config.protocol = val;
    api.saveGuiConfig(config);
    updateVisualState(currentState, currentStatus);
  });

  // 2. Obfuscation Profile Dropdown
  setupCustomDropdown('btn-obfs', 'panel-obfs', 'label-obfs', (val) => {
    config.noize = val;
    api.saveGuiConfig(config);
    updateVisualState(currentState, currentStatus);
  });

  // 3. Tor Bridge Region Dropdown
  setupCustomDropdown('btn-bridge', 'panel-bridge', 'label-bridge', (val) => {
    config.tor_country = val;
    api.saveGuiConfig(config);
  });

  // 4. Zero Trust Auth Dropdown
  const serviceFields = document.getElementById('auth-fields-service');
  const emailFields = document.getElementById('auth-fields-email');
  const accessFields = document.getElementById('auth-fields-access');

  setupCustomDropdown('btn-auth', 'panel-auth', 'label-auth', (val) => {
    if (!serviceFields || !emailFields || !accessFields) return;
    serviceFields.classList.add('hidden');
    emailFields.classList.add('hidden');
    accessFields.classList.add('hidden');

    if (val === 'service') {
      serviceFields.classList.remove('hidden');
    } else if (val === 'email') {
      emailFields.classList.remove('hidden');
    } else if (val === 'access') {
      accessFields.classList.remove('hidden');
    }
  });

  // Scan Mode Segmented Group
  const scanButtons = document.querySelectorAll('#scan-mode-group [data-scan]');
  scanButtons.forEach((btn) => {
    btn.addEventListener('click', () => {
      scanButtons.forEach((b) => {
        b.classList.remove('bg-[#24242a]', 'text-[#ffb691]', 'font-medium');
        b.classList.add('text-on-surface-variant');
      });
      btn.classList.add('bg-[#24242a]', 'text-[#ffb691]', 'font-medium');
      btn.classList.remove('text-on-surface-variant');

      const val = btn.getAttribute('data-scan');
      config.scan_mode = val;
      api.saveGuiConfig(config);
      updateVisualState(currentState, currentStatus);
    });
  });

  // Tunnel Mode (Proxy SOCKS5 vs System-wide TUN)
  const tunnelModeGroup = document.querySelectorAll('#tunnel-mode-group [data-mode]');
  const tunnelModeHint = document.getElementById('tunnel-mode-hint');

  function setTunnelMode(mode) {
    config.tunnel_mode = mode;
    tunnelModeGroup.forEach((b) => {
      if (b.getAttribute('data-mode') === mode) {
        b.className = 'px-3.5 py-1.5 rounded-md text-xs font-medium bg-[#222228] text-on-surface border border-[#393944]/50 transition-colors';
      } else {
        b.className = 'px-3.5 py-1.5 rounded-md text-xs font-medium text-on-surface-variant hover:text-on-surface transition-colors';
      }
    });

    if (tunnelModeHint) {
      if (mode === 'system-wide') {
        tunnelModeHint.textContent = 'System-wide Proxy Active (Routes all Windows apps & browsers)';
        tunnelModeHint.className = 'text-[10px] text-[#f2711c] font-mono';
      } else {
        tunnelModeHint.textContent = 'Loopback proxy active on 127.0.0.1:1819';
        tunnelModeHint.className = 'text-[10px] text-[#2dd4bf] font-mono';
      }
    }
    api.saveGuiConfig(config);
  }

  tunnelModeGroup.forEach((btn) => {
    btn.addEventListener('click', () => {
      const mode = btn.getAttribute('data-mode');
      setTunnelMode(mode);
    });
  });

  // Interactive Switch Helper
  function updateToggleUI(btnId, checked) {
    const btn = document.getElementById(btnId);
    if (!btn) return;
    btn.setAttribute('aria-checked', String(checked));
    const thumb = btn.firstElementChild;
    if (checked) {
      btn.classList.remove('bg-[#222228]', 'justify-start');
      btn.classList.add('bg-[#f2711c]', 'justify-end');
      if (thumb) {
        thumb.classList.remove('bg-[#393944]');
        thumb.classList.add('bg-[#0d0d0f]');
      }
    } else {
      btn.classList.remove('bg-[#f2711c]', 'justify-end');
      btn.classList.add('bg-[#222228]', 'justify-start');
      if (thumb) {
        thumb.classList.remove('bg-[#0d0d0f]');
        thumb.classList.add('bg-[#393944]');
      }
    }
  }

  function setupToggleSwitch(btnId, initialChecked, onToggle) {
    const btn = document.getElementById(btnId);
    if (!btn) return;

    updateToggleUI(btnId, initialChecked);

    btn.addEventListener('click', () => {
      const cur = btn.getAttribute('aria-checked') === 'true';
      const next = !cur;
      updateToggleUI(btnId, next);
      if (onToggle) onToggle(next);
    });
  }

  // 0. Smart Auto Connection Mode Toggle
  const rowProtocol = document.getElementById('row-protocol');
  const rowScanMode = document.getElementById('row-scan-mode');

  function updateAutoConnectUI(isAuto) {
    config.auto_connect = isAuto;
    if (rowProtocol) {
      if (isAuto) {
        rowProtocol.classList.add('opacity-50', 'pointer-events-none');
      } else {
        rowProtocol.classList.remove('opacity-50', 'pointer-events-none');
      }
    }
    if (rowScanMode) {
      if (isAuto) {
        rowScanMode.classList.add('opacity-50', 'pointer-events-none');
      } else {
        rowScanMode.classList.remove('opacity-50', 'pointer-events-none');
      }
    }
    if (ftActiveEngine) {
      ftActiveEngine.textContent = isAuto
        ? 'AUTO · Smart Routing Active'
        : `${config.protocol.toUpperCase()} · ${config.scan_mode.toUpperCase()} Scan`;
    }
    api.saveGuiConfig(config);
  }

  setupToggleSwitch('toggle-auto-connect', config.auto_connect, (checked) => {
    updateAutoConnectUI(checked);
  });

  // 1. Auto System Proxy Toggle
  setupToggleSwitch('toggle-sysproxy', config.auto_system_proxy, (checked) => {
    config.auto_system_proxy = checked;
    api.saveGuiConfig(config);
  });

  // 2. Startup Toggle — also writes the HKCU Run entry so it takes effect now
  setupToggleSwitch('toggle-startup', config.launch_at_startup, (checked) => {
    config.launch_at_startup = checked;
    api.setLaunchAtStartup(checked, config);
  });

  // 3. Minimized Toggle
  setupToggleSwitch('toggle-minimized', config.start_minimized, (checked) => {
    config.start_minimized = checked;
    api.saveGuiConfig(config);
  });

  // 4. Close to Tray Toggle
  setupToggleSwitch('toggle-closetray', config.close_to_tray, (checked) => {
    config.close_to_tray = checked;
    api.saveGuiConfig(config);
  });

  // 5. Tor Integration Toggle
  const torSubOptions = document.getElementById('tor-sub-options');
  setupToggleSwitch('toggle-tor', config.tor_enabled, (checked) => {
    config.tor_enabled = checked;
    if (torSubOptions) {
      torSubOptions.style.opacity = checked ? '1' : '0.35';
      torSubOptions.style.pointerEvents = checked ? 'auto' : 'none';
    }
    api.saveGuiConfig(config);
  });

  // Tor Mode Radio group
  document.querySelectorAll('input[name="tor-mode"]').forEach((radio) => {
    radio.addEventListener('change', (e) => {
      if (e.target.checked) {
        config.tor_mode = e.target.value;
        api.saveGuiConfig(config);
      }
    });
  });

  // 6. Tor Bridges Toggle
  const bridgeRegionRow = document.getElementById('bridge-region-row');
  setupToggleSwitch('toggle-tor-bridges', config.tor_bridges, (checked) => {
    config.tor_bridges = checked;
    if (bridgeRegionRow) {
      bridgeRegionRow.style.opacity = checked ? '1' : '0.5';
      bridgeRegionRow.style.pointerEvents = checked ? 'auto' : 'none';
    }
    api.saveGuiConfig(config);
  });

  // 6.5 Psiphon Integration Toggle
  const psiphonSubOptions = document.getElementById('psiphon-sub-options');
  setupToggleSwitch('toggle-psiphon', config.psiphon_enabled, (checked) => {
    config.psiphon_enabled = checked;
    if (psiphonSubOptions) {
      psiphonSubOptions.style.opacity = checked ? '1' : '0.4';
      psiphonSubOptions.style.pointerEvents = checked ? 'auto' : 'none';
    }
    api.saveGuiConfig(config);
  });

  // Psiphon Mode Radio group (carry / reach / psiphon-only)
  document.querySelectorAll('input[name="psiphon-mode"]').forEach((radio) => {
    radio.addEventListener('change', (e) => {
      if (e.target.checked) {
        config.psiphon_mode = e.target.value;
        api.saveGuiConfig(config);
      }
    });
  });

  // Psiphon tunnel shape dropdown (auto / cdn / direct)
  setupCustomDropdown('btn-psiphon-shape', 'panel-psiphon-shape', 'label-psiphon-shape', (val) => {
    config.psiphon_shape = val;
    syncFormToConfig();
    api.saveGuiConfig(config);
  });

  // 7. ClientHello Fragmentation Toggle (applies to the MASQUE HTTP/2 carrier)
  setupToggleSwitch('toggle-fragment', config.fragment, (checked) => {
    config.fragment = checked;
    api.saveGuiConfig(config);
  });

  // Persist the advanced engine fields whenever they change.
  [
    'cfg-bind', 'cfg-dns', 'cfg-bypass', 'cfg-force', 'cfg-block',
    'cfg-upstream', 'cfg-peer', 'cfg-wiw-outer', 'cfg-wiw-inner',
    'cfg-mim-outer', 'cfg-mim-inner', 'cfg-tor-bind',
    'cfg-tor-bridge-lines',
    'cfg-psiphon-region', 'cfg-psiphon-cdn-ips', 'cfg-psiphon-cdn-sni',
    'cfg-psiphon-bin', 'cfg-psiphon-http',
    'cfg-fragment-size', 'cfg-fragment-delay', 'cfg-team',
    'auth-email', 'auth-access-token', 'auth-client-id', 'auth-client-secret'
  ].forEach((id) => {
    const el = document.getElementById(id);
    if (!el) return;
    el.addEventListener('change', () => {
      syncFormToConfig();
      api.saveGuiConfig(config);
    });
  });

  // Zero Trust one-time-code dialog. The engine asks on a log line and waits
  // on its stdin; submitting writes the code back to the running process.
  const otpModal = document.getElementById('otp-modal');
  const otpInput = document.getElementById('otp-input');
  const otpSubtitle = document.getElementById('otp-modal-subtitle');
  const otpHint = document.getElementById('otp-modal-hint');
  const otpSubmit = document.getElementById('otp-submit');
  const otpCancel = document.getElementById('otp-cancel');

  function openOtpModal(payload) {
    if (!otpModal) return;
    if (otpSubtitle) {
      otpSubtitle.textContent = `A login code was emailed to ${payload.email || 'you'} (attempt ${payload.attempt || 1})`;
    }
    if (otpHint) {
      otpHint.textContent = 'Enter the one-time code. Three attempts are allowed; the sign-in fails if the code never arrives.';
    }
    otpModal.classList.remove('hidden');
    setTimeout(() => {
      otpModal.classList.remove('opacity-0');
      if (otpInput) otpInput.focus();
    }, 10);
  }

  function closeOtpModal() {
    if (!otpModal) return;
    otpModal.classList.add('opacity-0');
    setTimeout(() => otpModal.classList.add('hidden'), 150);
    if (otpInput) otpInput.value = '';
  }

  async function submitOtp() {
    const code = (otpInput && otpInput.value.trim()) || '';
    if (!code) {
      if (otpInput) otpInput.focus();
      return;
    }
    try {
      await api.submitOtpCode(code);
      closeOtpModal();
    } catch (e) {
      if (otpHint) otpHint.textContent = `Could not deliver the code: ${e}`;
    }
  }

  if (otpSubmit) otpSubmit.addEventListener('click', submitOtp);
  if (otpCancel) otpCancel.addEventListener('click', closeOtpModal);
  if (otpInput) {
    otpInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') submitOtp();
    });
  }

  await api.onOtpRequest((payload) => {
    openOtpModal(payload || {});
  });
  await api.onTorAddr((payload) => {
    const addr = payload && typeof payload === 'object' ? payload.addr : payload;
    torAddr = typeof addr === 'string' ? addr.trim() : '';
    // The engine's "tor is ready" line can land just before or just after the
    // connected status, so a live badge is redrawn on either order.
    if (currentState === 'connected') updateVisualState('connected', currentStatus);
  });

  await api.onPsiphonAddr((payload) => {
    const addr = payload && typeof payload === 'object' ? payload.addr : payload;
    psiphonAddr = typeof addr === 'string' ? addr.trim() : '';
    // Same ordering tolerance as the tor badge.
    if (currentState === 'connected') updateVisualState('connected', currentStatus);
  });

  await api.onProgress((payload) => {
    const pct = payload && typeof payload === 'object' ? payload.percent : null;
    const stage = payload && typeof payload === 'object' ? payload.stage : payload;
    if (dialProgressPct && pct != null) {
      dialProgressPct.textContent = `${pct}%`;
    }
    if (currentState === 'connecting') {
      if (statusPrimary && stage) {
        statusPrimary.innerHTML = `${stage} · <span class="text-secondary font-mono font-medium">${pct || 0}%</span>`;
      }
    }
  });

  await api.onExitInfo((payload) => {
    if (payload && typeof payload === 'object') {
      if (payload.ip) currentStatus.exit_ip = payload.ip;
      if (payload.colo) currentStatus.colo = payload.colo;
      if (payload.loc) currentStatus.loc = payload.loc;
      if (payload.latency_ms) currentStatus.latency_ms = payload.latency_ms;
      if (currentState === 'connected') {
        updateVisualState('connected', currentStatus);
      }
    }
  });

  // Check for Updates Modal
  const btnCheckUpdates = document.getElementById('btn-check-updates');
  const iconCheckUpdates = document.getElementById('icon-check-updates');
  const textCheckUpdates = document.getElementById('text-check-updates');
  const updateModal = document.getElementById('update-modal');
  const updateModalDialog = document.getElementById('update-modal-dialog');
  const updateModalClose = document.getElementById('update-modal-close');
  const updateModalOk = document.getElementById('update-modal-ok');

  function openUpdateModal() {
    if (!updateModal) return;
    updateModal.classList.remove('hidden');
    setTimeout(() => {
      updateModal.classList.remove('opacity-0');
      if (updateModalDialog) updateModalDialog.classList.remove('scale-95');
    }, 10);
  }

  function closeUpdateModal() {
    if (!updateModal) return;
    updateModal.classList.add('opacity-0');
    if (updateModalDialog) updateModalDialog.classList.add('scale-95');
    setTimeout(() => {
      updateModal.classList.add('hidden');
    }, 180);
  }

  if (btnCheckUpdates) {
    btnCheckUpdates.addEventListener('click', async () => {
      if (isCheckingUpdates) return;
      isCheckingUpdates = true;
      if (iconCheckUpdates) iconCheckUpdates.classList.add('animate-spin');
      if (textCheckUpdates) textCheckUpdates.textContent = 'Checking...';

      try {
        const info = await api.checkForUpdates();
        const title = document.getElementById('update-modal-title');
        const body = document.getElementById('update-modal-body');
        const link = document.getElementById('update-modal-link');
        if (title) {
          title.textContent = info.update_available ? 'Update available' : 'You are up to date';
        }
        if (body) {
          body.textContent = info.update_available
            ? `You are on v${info.current}; v${info.latest} is out.`
            : `v${info.current} is the latest release.`;
        }
        if (link) {
          link.textContent = info.url;
          link.classList.toggle('hidden', !info.update_available);
        }
        openUpdateModal();
      } catch (e) {
        const title = document.getElementById('update-modal-title');
        const body = document.getElementById('update-modal-body');
        if (title) title.textContent = 'Update check failed';
        if (body) body.textContent = `Could not reach GitHub: ${e}`;
        openUpdateModal();
      } finally {
        isCheckingUpdates = false;
        if (iconCheckUpdates) iconCheckUpdates.classList.remove('animate-spin');
        if (textCheckUpdates) textCheckUpdates.textContent = 'Check for updates';
      }
    });
  }

  if (updateModalClose) updateModalClose.addEventListener('click', closeUpdateModal);
  if (updateModalOk) updateModalOk.addEventListener('click', closeUpdateModal);

  // Restore Defaults
  function restoreDefaults() {
    config = {
      protocol: 'masque',
      socks_port: 1819,
      http_port: 1820,
      scan_mode: 'balanced',
      ip_family: 'v4',
      noize: 'firewall',
      peer: '',
      dns: '1.1.1.1, 1.0.0.1',
      upstream: '',
      auto_system_proxy: true,
      bypass_list: '<local>;localhost;127.*;10.*;192.168.*;172.16.*;*.ir;bank.ir',
      route_direct: '',
      route_block: '',
      tunnel_mode: 'proxy',
      auto_connect: true,
      tor_enabled: false,
      tor_mode: 'carry',
      tor_bridges: false,
      tor_country: 'auto',
      tor_bind: '',
      psiphon_enabled: false,
      psiphon_mode: 'carry',
      psiphon_region: '',
      psiphon_shape: 'auto',
      psiphon_cdn_ips: '',
      psiphon_cdn_sni: '',
      psiphon_bin: '',
      psiphon_http: '',
      wiw_outer: '',
      wiw_inner: '',
      mim_outer: '',
      mim_inner: '',
      fragment: false,
      fragment_size: '16-32',
      fragment_delay: '2-10',
      launch_at_startup: false,
      start_minimized: false,
      close_to_tray: true,
      team: '',
      access_email: '',
      access_token: '',
      access_id: '',
      access_secret: ''
    };
    populateConfigToForm();
    api.saveGuiConfig(config);
    updateVisualState(currentState, currentStatus);
  }

  const btnRestoreBasic = document.getElementById('btn-restore-basic');
  const btnRestoreAdvanced = document.getElementById('btn-restore-advanced');
  if (btnRestoreBasic) btnRestoreBasic.addEventListener('click', restoreDefaults);
  if (btnRestoreAdvanced) btnRestoreAdvanced.addEventListener('click', restoreDefaults);

  // Sync Input Fields to Config
  function syncFormToConfig() {
    if (!config.http_port) {
      config.http_port = 1820;
    }

    const bindInput = document.getElementById('cfg-bind');
    if (bindInput) {
      const parts = bindInput.value.trim().split(':');
      if (parts.length === 2 && !isNaN(parseInt(parts[1], 10))) {
        config.socks_port = parseInt(parts[1], 10);
      }
    }

    const dnsInput = document.getElementById('cfg-dns');
    if (dnsInput) config.dns = dnsInput.value.trim();

    const bypassInput = document.getElementById('cfg-bypass');
    if (bypassInput) config.bypass_list = bypassInput.value.trim();

    const forceInput = document.getElementById('cfg-force');
    if (forceInput) config.route_direct = forceInput.value.trim();

    const blockInput = document.getElementById('cfg-block');
    if (blockInput) config.route_block = blockInput.value.trim();

    const upstreamInput = document.getElementById('cfg-upstream');
    if (upstreamInput) config.upstream = upstreamInput.value.trim();

    const peerInput = document.getElementById('cfg-peer');
    if (peerInput) config.peer = peerInput.value.trim();

    const wiwOuter = document.getElementById('cfg-wiw-outer');
    if (wiwOuter) config.wiw_outer = wiwOuter.value.trim();
    const wiwInner = document.getElementById('cfg-wiw-inner');
    if (wiwInner) config.wiw_inner = wiwInner.value.trim();
    const mimOuter = document.getElementById('cfg-mim-outer');
    if (mimOuter) config.mim_outer = mimOuter.value.trim();
    const mimInner = document.getElementById('cfg-mim-inner');
    if (mimInner) config.mim_inner = mimInner.value.trim();

    const torBind = document.getElementById('cfg-tor-bind');
    if (torBind) config.tor_bind = torBind.value.trim();
    const torBridgeLines = document.getElementById('cfg-tor-bridge-lines');
    if (torBridgeLines) config.tor_bridge_lines = torBridgeLines.value.trim();

    const psRegion = document.getElementById('cfg-psiphon-region');
    if (psRegion) config.psiphon_region = psRegion.value.trim();

    const psCdnIps = document.getElementById('cfg-psiphon-cdn-ips');
    if (psCdnIps) config.psiphon_cdn_ips = psCdnIps.value.trim();

    const psCdnSni = document.getElementById('cfg-psiphon-cdn-sni');
    if (psCdnSni) config.psiphon_cdn_sni = psCdnSni.value.trim();

    const psBin = document.getElementById('cfg-psiphon-bin');
    if (psBin) config.psiphon_bin = psBin.value.trim();

    const psHttp = document.getElementById('cfg-psiphon-http');
    if (psHttp) config.psiphon_http = psHttp.value.trim();

    const fragSize = document.getElementById('cfg-fragment-size');
    if (fragSize) config.fragment_size = fragSize.value.trim();
    const fragDelay = document.getElementById('cfg-fragment-delay');
    if (fragDelay) config.fragment_delay = fragDelay.value.trim();

    const teamInput = document.getElementById('cfg-team');
    if (teamInput) config.team = teamInput.value.trim();

    const emailInput = document.getElementById('auth-email');
    if (emailInput) config.access_email = emailInput.value.trim();

    const tokenInput = document.getElementById('auth-access-token');
    if (tokenInput) config.access_token = tokenInput.value.trim();

    const idInput = document.getElementById('auth-client-id');
    if (idInput) config.access_id = idInput.value.trim();

    const secInput = document.getElementById('auth-client-secret');
    if (secInput) config.access_secret = secInput.value.trim();
  }

  // Populate Config into Form Inputs
  function populateConfigToForm() {
    const setVal = (id, val) => {
      const el = document.getElementById(id);
      if (el && val !== undefined && val !== null) el.value = val;
    };

    setVal('cfg-bind', `127.0.0.1:${config.socks_port}`);
    setVal('cfg-dns', config.dns || '1.1.1.1, 1.0.0.1');
    setVal('cfg-bypass', config.bypass_list);
    setVal('cfg-force', config.route_direct || '');
    setVal('cfg-block', config.route_block || '');
    setVal('cfg-upstream', config.upstream || '');
    setVal('cfg-peer', config.peer || '');
    setVal('cfg-wiw-outer', config.wiw_outer || '');
    setVal('cfg-wiw-inner', config.wiw_inner || '');
    setVal('cfg-mim-outer', config.mim_outer || '');
    setVal('cfg-mim-inner', config.mim_inner || '');
    setVal('cfg-tor-bind', config.tor_bind || '');
    setVal('cfg-tor-bridge-lines', config.tor_bridge_lines || '');
    setVal('cfg-psiphon-region', config.psiphon_region || '');
    setVal('cfg-psiphon-cdn-ips', config.psiphon_cdn_ips || '');
    setVal('cfg-psiphon-cdn-sni', config.psiphon_cdn_sni || '');
    setVal('cfg-psiphon-bin', config.psiphon_bin || '');
    setVal('cfg-psiphon-http', config.psiphon_http || '');
    setVal('cfg-fragment-size', config.fragment_size || '16-32');
    setVal('cfg-fragment-delay', config.fragment_delay || '2-10');
    setVal('cfg-team', config.team || '');
    setVal('auth-email', config.access_email || '');
    setVal('auth-access-token', config.access_token || '');
    setVal('auth-client-id', config.access_id || '');
    setVal('auth-client-secret', config.access_secret || '');

    // Protocol label
    const labelProto = document.getElementById('label-proto');
    if (labelProto) labelProto.textContent = getProtocolDisplayName(config.protocol);

    // Obfuscation label
    const labelObfs = document.getElementById('label-obfs');
    if (labelObfs) labelObfs.textContent = config.noize.charAt(0).toUpperCase() + config.noize.slice(1);


    // Move each dropdown tick onto the saved value
    syncDropdownTick('panel-proto', config.protocol);
    syncDropdownTick('panel-obfs', config.noize);
    syncDropdownTick('panel-bridge', config.tor_country || 'auto');

    // Sync all toggle switch visual states
    updateToggleUI('toggle-psiphon', !!config.psiphon_enabled);
    updateToggleUI('toggle-tor', !!config.tor_enabled);
    updateToggleUI('toggle-tor-bridges', !!config.tor_bridges);
    updateToggleUI('toggle-fragment', !!config.fragment);
    updateToggleUI('toggle-sysproxy', config.auto_system_proxy !== false);
    updateToggleUI('toggle-startup', !!config.launch_at_startup);
    updateToggleUI('toggle-minimized', !!config.start_minimized);
    updateToggleUI('toggle-closetray', config.close_to_tray !== false);
    updateToggleUI('toggle-auto-connect', config.auto_connect !== false);

    if (torSubOptions) {
      torSubOptions.style.opacity = config.tor_enabled ? '1' : '0.35';
      torSubOptions.style.pointerEvents = config.tor_enabled ? 'auto' : 'none';
    }
    if (bridgeRegionRow) {
      bridgeRegionRow.style.opacity = config.tor_bridges ? '1' : '0.5';
      bridgeRegionRow.style.pointerEvents = config.tor_bridges ? 'auto' : 'none';
    }

    // Psiphon toggle + mode radios + shape dropdown reflect the saved config.
    if (psiphonSubOptions) {
      psiphonSubOptions.style.opacity = config.psiphon_enabled ? '1' : '0.4';
      psiphonSubOptions.style.pointerEvents = config.psiphon_enabled ? 'auto' : 'none';
    }
    document.querySelectorAll('input[name="psiphon-mode"]').forEach((radio) => {
      radio.checked = radio.value === config.psiphon_mode;
    });
    // The shape dropdown is a setupCustomDropdown, so the default header label
    // only reflects the initial "Auto" option; move the tick and label onto the
    // saved value. (setupToggleSwitch already set the toggle's aria-checked.)
    syncDropdownTick('panel-psiphon-shape', config.psiphon_shape);
    const psShapeMatch = document
      .querySelector('#panel-psiphon-shape [data-value="' + config.psiphon_shape + '"] span');
    const labelPsShape = document.getElementById('label-psiphon-shape');
    if (labelPsShape && psShapeMatch) labelPsShape.textContent = psShapeMatch.textContent;
    // Scan buttons
    scanButtons.forEach((b) => {
      if (b.getAttribute('data-scan') === config.scan_mode) {
        b.classList.add('bg-[#24242a]', 'text-[#ffb691]', 'font-medium');
        b.classList.remove('text-on-surface-variant');
      } else {
        b.classList.remove('bg-[#24242a]', 'text-[#ffb691]', 'font-medium');
        b.classList.add('text-on-surface-variant');
      }
    });

    setTunnelMode(config.tunnel_mode || 'proxy');
    updateAutoConnectUI(config.auto_connect !== false);
  }

  // Initialize Form
  populateConfigToForm();

  // -------------------------------------------------------------------------
  // 7. REAL-TIME EVENT STREAMING (Backend Sync)
  // -------------------------------------------------------------------------
  await api.onLog((entry) => {
    appendLogLine(entry);
  });

  await api.onStatus((statusPayload) => {
    currentStatus = statusPayload;
    if (statusPayload.state === 'connected') {
      updateVisualState('connected', statusPayload);
    } else if (statusPayload.state === 'connecting' || statusPayload.state === 'scanning' || statusPayload.state === 'reconnecting') {
      updateVisualState('connecting', statusPayload);
    } else if (statusPayload.state === 'error') {
      updateVisualState('error', statusPayload);
    } else {
      updateVisualState('idle', statusPayload);
    }
  });

  // Initial Status Check
  try {
    const initSt = await api.getStatus();
    if (initSt && initSt.state === 'connected') {
      updateVisualState('connected', initSt);
    } else {
      updateVisualState('idle');
    }
  } catch (_) {
    updateVisualState('idle');
  }

  // Periodic Latency and Trace Polling
  setInterval(async () => {
    if (currentState === 'connected') {
      try {
        const ms = await api.testLatency(config.socks_port);
        currentStatus.latency_ms = ms;
        updateVisualState('connected', currentStatus);
      } catch (_) {}
    }
  }, 10000);
  setInterval(() => {
    if (currentState === 'connected') refreshTrace();
  }, 25000);
});
