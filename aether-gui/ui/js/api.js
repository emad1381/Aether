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
  }
};
