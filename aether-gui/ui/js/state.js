import { api } from './api.js';

export class StateManager {
  constructor(uiCallbacks) {
    this.ui = uiCallbacks;
    this.status = {
      state: 'disconnected',
      latency_ms: null,
      uptime_secs: 0,
      socks_endpoint: '127.0.0.1:1819',
      system_proxy_active: false,
      exit_ip: null,
      colo: null
    };
    this.timerInterval = null;
    this.pingInterval = null;
  }

  async init() {
    // Listen to backend status events
    await api.onStatus((newStatus) => {
      this.updateStatus(newStatus);
    });

    // Initial poll
    const initial = await api.getStatus();
    this.updateStatus(initial);
  }

  updateStatus(newStatus) {
    const prevState = this.status.state;
    this.status = { ...this.status, ...newStatus };

    if (this.status.state === 'connected') {
      if (prevState !== 'connected') {
        this.startTimers();
        this.fetchTrace();
      }
    } else {
      this.stopTimers();
    }

    this.ui.renderStatus(this.status);
  }

  startTimers() {
    this.stopTimers();

    // Uptime ticker
    this.timerInterval = setInterval(() => {
      this.status.uptime_secs++;
      this.ui.renderUptime(this.status.uptime_secs);
    }, 1000);

    // Periodic ping measurement (every 12 seconds)
    this.pingInterval = setInterval(async () => {
      if (this.status.state === 'connected') {
        const socksPort = parseInt(this.status.socks_endpoint.split(':')[1] || '1819', 10);
        try {
          const lat = await api.testLatency(socksPort);
          this.status.latency_ms = lat;
          this.ui.renderLatency(lat);
        } catch (_) {}
      }
    }, 12000);
  }

  stopTimers() {
    if (this.timerInterval) clearInterval(this.timerInterval);
    if (this.pingInterval) clearInterval(this.pingInterval);
    this.timerInterval = null;
    this.pingInterval = null;
  }

  async fetchTrace() {
    const socksPort = parseInt(this.status.socks_endpoint.split(':')[1] || '1819', 10);
    try {
      const trace = await api.fetchTraceInfo(socksPort);
      this.status.exit_ip = trace.ip;
      this.status.colo = trace.colo;
      this.ui.renderExitInfo(trace);
    } catch (_) {}
  }
}
