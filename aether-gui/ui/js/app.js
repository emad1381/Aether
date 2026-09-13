import { api } from './api.js';
import { Terminal } from './terminal.js';
import { StateManager } from './state.js';

document.addEventListener('DOMContentLoaded', async () => {
  // DOM Elements
  const powerBtnSection = document.getElementById('powerBtnSection');
  const powerBtn = document.getElementById('powerBtn');
  const statusBadge = document.getElementById('statusBadge');
  const statusText = document.getElementById('statusText');
  const metricLatency = document.getElementById('metricLatency');
  const metricUptime = document.getElementById('metricUptime');
  const metricProtocol = document.getElementById('metricProtocol');
  const exitIpEl = document.getElementById('exitIp');
  const exitColoEl = document.getElementById('exitColo');
  const quickProxyToggle = document.getElementById('quickProxyToggle');
  const consoleDrawer = document.getElementById('consoleDrawer');
  const consoleHeader = document.getElementById('consoleHeader');
  const consoleLogs = document.getElementById('consoleLogs');
  const clearLogsBtn = document.getElementById('clearLogsBtn');

  // Terminal Setup
  const terminal = new Terminal(consoleLogs);
  await api.onLog((entry) => {
    terminal.append(entry);
  });

  if (clearLogsBtn) {
    clearLogsBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      terminal.clear();
    });
  }

  // Console Drawer Expand/Collapse
  if (consoleHeader) {
    consoleHeader.addEventListener('click', () => {
      consoleDrawer.classList.toggle('expanded');
    });
  }

  // Active Configuration State
  let config = {
    protocol: 'masque',
    socks_port: 1819,
    http_port: null,
    scan_mode: 'balanced',
    ip_family: 'v4',
    noize: 'firewall',
    peer: '',
    wiw_outer: '',
    wiw_inner: '',
    mim_outer: '',
    mim_inner: '',
    fragment: false,
    fragment_size: '16-32',
    fragment_delay: '2-10',
    keepalive: 5,
    dns: '',
    upstream: '',
    auto_system_proxy: true,
    bypass_list: '<local>;localhost;127.*;10.*;192.168.*;172.16.*;*.ir',
    route_direct: '',
    route_block: '',
    team: '',
    access_email: '',
    access_token: '',
    access_id: '',
    access_secret: ''
  };

  // Load Saved Config if available
  const saved = await api.getSavedConfig();
  if (saved) {
    config = { ...config, ...saved };
  }

  // Helper: Format Uptime
  function formatUptime(totalSecs) {
    const hrs = Math.floor(totalSecs / 3600);
    const mins = Math.floor((totalSecs % 3600) / 60);
    const secs = totalSecs % 60;
    return `${hrs.toString().padStart(2, '0')}:${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
  }

  // Helper: Format Protocol Name
  function formatProtocol(proto) {
    switch (proto) {
      case 'masque': return 'MASQUE (H3)';
      case 'masque-h2': return 'MASQUE (H2)';
      case 'wg': return 'WireGuard';
      case 'gool': return 'WARP-in-WARP';
      case 'mim': return 'MASQUE-in-MASQUE';
      case 'tor': return 'Tor (Chain)';
      case 'tor-reverse': return 'Tor (Reverse)';
      case 'tor-only': return 'Tor Only';
      default: return proto.toUpperCase();
    }
  }

  // UI Callback Renderers for StateManager
  const uiCallbacks = {
    renderStatus(status) {
      powerBtnSection.className = `power-section ${status.state}`;
      statusBadge.className = `status-badge ${status.state}`;

      switch (status.state) {
        case 'connected':
          statusText.textContent = 'Connected';
          break;
        case 'scanning':
          statusText.textContent = 'Scanning Routes...';
          break;
        case 'connecting':
          statusText.textContent = 'Connecting...';
          break;
        case 'reconnecting':
          statusText.textContent = 'Reconnecting...';
          break;
        case 'error':
          statusText.textContent = 'Connection Error';
          break;
        default:
          statusText.textContent = 'Disconnected';
      }

      metricProtocol.textContent = formatProtocol(config.protocol);

      if (status.state !== 'connected') {
        metricLatency.textContent = '-- ms';
        metricLatency.className = 'metric-value';
        metricUptime.textContent = '00:00:00';
        exitIpEl.textContent = 'Not connected';
        exitColoEl.textContent = '--';
      }

      if (quickProxyToggle) {
        quickProxyToggle.checked = status.system_proxy_active;
      }
    },

    renderUptime(secs) {
      metricUptime.textContent = formatUptime(secs);
    },

    renderLatency(ms) {
      metricLatency.textContent = `${ms} ms`;
      if (ms < 100) {
        metricLatency.className = 'metric-value good';
      } else if (ms < 250) {
        metricLatency.className = 'metric-value medium';
      } else {
        metricLatency.className = 'metric-value bad';
      }
    },

    renderExitInfo(trace) {
      exitIpEl.textContent = trace.ip || 'Cloudflare Edge';
      exitColoEl.textContent = `${trace.colo || '--'} · ${trace.loc || '--'}`;
    }
  };

  const stateManager = new StateManager(uiCallbacks);
  await stateManager.init();

  // Navigation Tabs Switching
  const navItems = document.querySelectorAll('.nav-item');
  const viewSections = document.querySelectorAll('.view-section');

  navItems.forEach((item) => {
    item.addEventListener('click', () => {
      const targetView = item.getAttribute('data-view');
      navItems.forEach((n) => n.classList.remove('active'));
      viewSections.forEach((s) => s.classList.remove('active'));

      item.classList.add('active');
      const activeSec = document.getElementById(`view-${targetView}`);
      if (activeSec) activeSec.classList.add('active');
    });
  });

  // Power Button Click Handler
  powerBtn.addEventListener('click', async () => {
    const currState = stateManager.status.state;
    if (currState === 'connected' || currState === 'connecting' || currState === 'scanning') {
      try {
        await api.stopTunnel();
      } catch (err) {
        console.error('Stop error:', err);
      }
    } else {
      try {
        // Collect form values into config
        syncFormToConfig();
        await api.saveGuiConfig(config);
        await api.startTunnel(config);
      } catch (err) {
        alert(`Failed to start tunnel: ${err}`);
      }
    }
  });

  // Quick Proxy Toggle
  if (quickProxyToggle) {
    quickProxyToggle.addEventListener('change', async () => {
      config.auto_system_proxy = quickProxyToggle.checked;
      try {
        const active = await api.toggleProxy(quickProxyToggle.checked, config);
        stateManager.status.system_proxy_active = active;
        uiCallbacks.renderStatus(stateManager.status);
      } catch (err) {
        alert(`System proxy error: ${err}`);
      }
    });
  }

  // Radio Cards Generic Setup
  function setupRadioCards(containerSelector, configKey, onSelect) {
    const container = document.querySelector(containerSelector);
    if (!container) return;
    const cards = container.querySelectorAll('.radio-card');
    cards.forEach((card) => {
      card.addEventListener('click', () => {
        cards.forEach((c) => c.classList.remove('active'));
        card.classList.add('active');
        const val = card.getAttribute('data-value');
        config[configKey] = val;
        api.saveGuiConfig(config);
        if (onSelect) onSelect(val);
      });
    });
  }

  setupRadioCards('#protocolCards', 'protocol', (val) => {
    metricProtocol.textContent = formatProtocol(val);
    const h2Opts = document.getElementById('masqueH2Options');
    if (h2Opts) {
      h2Opts.style.display = val === 'masque-h2' ? 'block' : 'none';
    }
  });

  setupRadioCards('#scanModeCards', 'scan_mode');
  setupRadioCards('#noizeCards', 'noize');
  setupRadioCards('#ipFamilyCards', 'ip_family');

  // Form Binding: Sync UI Form -> config object
  function syncFormToConfig() {
    const socksInput = document.getElementById('inputSocksPort');
    if (socksInput) config.socks_port = parseInt(socksInput.value, 10) || 1819;

    const httpInput = document.getElementById('inputHttpPort');
    if (httpInput) {
      config.http_port = httpInput.value.trim() ? parseInt(httpInput.value, 10) : null;
    }

    const peerInput = document.getElementById('inputPeer');
    if (peerInput) config.peer = peerInput.value.trim();

    const dnsInput = document.getElementById('inputDns');
    if (dnsInput) config.dns = dnsInput.value.trim();

    const upstreamInput = document.getElementById('inputUpstream');
    if (upstreamInput) config.upstream = upstreamInput.value.trim();

    const bypassInput = document.getElementById('inputBypass');
    if (bypassInput) config.bypass_list = bypassInput.value.trim();

    const directInput = document.getElementById('inputRouteDirect');
    if (directInput) config.route_direct = directInput.value.trim();

    const blockInput = document.getElementById('inputRouteBlock');
    if (blockInput) config.route_block = blockInput.value.trim();

    const teamInput = document.getElementById('inputTeam');
    if (teamInput) config.team = teamInput.value.trim();

    const emailInput = document.getElementById('inputAccessEmail');
    if (emailInput) config.access_email = emailInput.value.trim();

    const tokenInput = document.getElementById('inputAccessToken');
    if (tokenInput) config.access_token = tokenInput.value.trim();

    const fragmentCheck = document.getElementById('checkFragment');
    if (fragmentCheck) config.fragment = fragmentCheck.checked;
  }

  // Form Binding: Populate config object -> UI Form
  function populateConfigToForm() {
    // Protocol cards
    const protoCard = document.querySelector(`#protocolCards .radio-card[data-value="${config.protocol}"]`);
    if (protoCard) protoCard.classList.add('active');

    const h2Opts = document.getElementById('masqueH2Options');
    if (h2Opts) h2Opts.style.display = config.protocol === 'masque-h2' ? 'block' : 'none';

    // Scan cards
    const scanCard = document.querySelector(`#scanModeCards .radio-card[data-value="${config.scan_mode}"]`);
    if (scanCard) scanCard.classList.add('active');

    // Noise cards
    const noizeCard = document.querySelector(`#noizeCards .radio-card[data-value="${config.noize}"]`);
    if (noizeCard) noizeCard.classList.add('active');

    // IP family
    const ipCard = document.querySelector(`#ipFamilyCards .radio-card[data-value="${config.ip_family}"]`);
    if (ipCard) ipCard.classList.add('active');

    // Inputs
    const setVal = (id, val) => {
      const el = document.getElementById(id);
      if (el && val !== undefined && val !== null) el.value = val;
    };

    setVal('inputSocksPort', config.socks_port);
    setVal('inputHttpPort', config.http_port || '');
    setVal('inputPeer', config.peer || '');
    setVal('inputDns', config.dns || '');
    setVal('inputUpstream', config.upstream || '');
    setVal('inputBypass', config.bypass_list);
    setVal('inputRouteDirect', config.route_direct || '');
    setVal('inputRouteBlock', config.route_block || '');
    setVal('inputTeam', config.team || '');
    setVal('inputAccessEmail', config.access_email || '');
    setVal('inputAccessToken', config.access_token || '');

    const fragmentCheck = document.getElementById('checkFragment');
    if (fragmentCheck) fragmentCheck.checked = !!config.fragment;

    const autoProxyCheck = document.getElementById('checkAutoProxy');
    if (autoProxyCheck) autoProxyCheck.checked = !!config.auto_system_proxy;

    metricProtocol.textContent = formatProtocol(config.protocol);
  }

  populateConfigToForm();
});
