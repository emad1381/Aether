// Aether Tauri v2 IPC Bridge

const hasTauri = () => typeof window.__TAURI__ !== 'undefined' && window.__TAURI__.core;

export const api = {
  async startTunnel(cfg) {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('start_tunnel', { cfg });
    }
    console.log('[Dev Mode] startTunnel:', cfg);
    return Promise.resolve();
  },

  async stopTunnel() {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('stop_tunnel');
    }
    console.log('[Dev Mode] stopTunnel');
    return Promise.resolve();
  },

  async getStatus() {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('get_status');
    }
    return {
      state: 'disconnected',
      latency_ms: null,
      uptime_secs: 0,
      socks_endpoint: '127.0.0.1:1819',
      system_proxy_active: false,
      exit_ip: null,
      colo: null,
      error_message: null
    };
  },

  async toggleProxy(enable, cfg) {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('toggle_proxy', { enable, cfg });
    }
    console.log('[Dev Mode] toggleProxy:', enable);
    return enable;
  },

  async testLatency(socksPort) {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('test_latency', { socksPort });
    }
    return Math.floor(60 + Math.random() * 40);
  },

  async fetchTraceInfo(socksPort) {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('fetch_trace_info', { socksPort });
    }
    return {
      ip: '188.114.96.12',
      loc: 'DE',
      colo: 'FRA',
      warp: 'on'
    };
  },

  async getSavedConfig() {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('get_saved_config');
    }
    return null;
  },

  async saveGuiConfig(cfg) {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('save_gui_config', { cfg });
    }
    localStorage.setItem('aether_config', JSON.stringify(cfg));
    return Promise.resolve();
  },

  async minimizeWindow() {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('minimize_window');
    }
  },

  async maximizeWindow() {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('maximize_window');
    }
  },

  async closeWindow() {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('close_window');
    }
  },

  // Hard exit for the updater: never resolves, the process goes away under us.
  async quitForUpdate() {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('quit_for_update');
    }
    return Promise.reject('installs only run inside the desktop app');
  },

  async startDragging() {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('start_dragging');
    }
  },

  async onLog(callback) {
    if (hasTauri() && window.__TAURI__.event) {
      return await window.__TAURI__.event.listen('aether-log', (event) => {
        callback(event.payload);
      });
    }
  },

  async onStatus(callback) {
    if (hasTauri() && window.__TAURI__.event) {
      return await window.__TAURI__.event.listen('aether-status', (event) => {
        callback(event.payload);
      });
    }
  },

  async onOtpRequest(callback) {
    if (hasTauri() && window.__TAURI__.event) {
      return await window.__TAURI__.event.listen('aether-otp-request', (event) => {
        callback(event.payload);
      });
    }
  },

  async onTorAddr(callback) {
    if (hasTauri() && window.__TAURI__.event) {
      return await window.__TAURI__.event.listen('aether-tor-addr', (event) => {
        callback(event.payload);
      });
    }
  },

  async scanCdnEdges() {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('scan_cdn_edges');
    }
    return { edges: [], ips: '', sni: '', tested: 0, reachable: 0 };
  },

  async onCdnScan(callback) {
    if (hasTauri() && window.__TAURI__.event) {
      return await window.__TAURI__.event.listen('aether-cdn-scan', (event) => {
        callback(event.payload);
      });
    }
  },

  async onPsiphonAddr(callback) {
    if (hasTauri() && window.__TAURI__.event) {
      return await window.__TAURI__.event.listen('aether-psiphon-addr', (event) => {
        callback(event.payload);
      });
    }
  },

  async onProgress(callback) {
    if (hasTauri() && window.__TAURI__.event) {
      return await window.__TAURI__.event.listen('aether-progress', (event) => {
        callback(event.payload);
      });
    }
  },

  async onExitInfo(callback) {
    if (hasTauri() && window.__TAURI__.event) {
      return await window.__TAURI__.event.listen('aether-exit-info', (event) => {
        callback(event.payload);
      });
    }
  },

  async submitOtpCode(code) {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('team_otp_submit', { code });
    }
    console.log('[Dev Mode] submitOtpCode');
    return Promise.resolve();
  },

  async checkForUpdates() {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('check_for_updates');
    }
    // Dev/preview fallback: the same real GitHub check, done from the page.
    const release = await fetch('https://api.github.com/repos/emad1381/Aether/releases/latest')
      .then((r) => r.json());
    const latest = String(release.tag_name || '').replace(/^v/, '');
    const asset = (release.assets || []).find((a) => a.name === 'aether-windows-x86_64-gui.zip');
    const current = '2.1.6';
    const parse = (v) => v.split('.').map((p) => parseInt(p, 10) || 0);
    const [la, lb, lc] = parse(latest);
    const [ca, cb, cc] = parse(current);
    const newer = la > ca || (la === ca && (lb > cb || (lb === cb && lc > cc)));
    return {
      current,
      latest,
      update_available: !!latest && newer,
      url: release.html_url || 'https://github.com/emad1381/Aether/releases',
      asset_url: asset ? asset.browser_download_url : ''
    };
  },

  async downloadUpdate() {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('download_update');
    }
    return Promise.reject('downloads install only inside the desktop app');
  },

  async onUpdateProgress(callback) {
    if (hasTauri() && window.__TAURI__.event) {
      return await window.__TAURI__.event.listen('aether-update-progress', (event) => {
        callback(event.payload);
      });
    }
  },

  async setLaunchAtStartup(enable, cfg) {
    if (hasTauri()) {
      return await window.__TAURI__.core.invoke('set_launch_at_startup', { enable, cfg });
    }
    console.log('[Dev Mode] setLaunchAtStartup:', enable);
    return Promise.resolve();
  }
};
