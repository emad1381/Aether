// Aether Live Console Terminal

const MAX_LOG_LINES = 800;

export class Terminal {
  constructor(containerElement) {
    this.container = containerElement;
    this.autoScroll = true;
    this.linesCount = 0;

    this.container.addEventListener('scroll', () => {
      const atBottom = this.container.scrollHeight - this.container.scrollTop <= this.container.clientHeight + 40;
      this.autoScroll = atBottom;
    });
  }

  append(entry) {
    const lineEl = document.createElement('div');
    lineEl.className = 'log-line';

    const timeEl = document.createElement('span');
    timeEl.className = 'log-time';
    timeEl.textContent = `[${entry.timestamp || ''}]`;

    const levelEl = document.createElement('span');
    const lvl = (entry.level || 'INFO').toLowerCase();
    levelEl.className = `log-level-${lvl}`;
    levelEl.textContent = entry.level || 'INFO';

    const msgEl = document.createElement('span');
    msgEl.className = 'log-msg';
    msgEl.textContent = entry.message || '';

    lineEl.appendChild(timeEl);
    lineEl.appendChild(levelEl);
    lineEl.appendChild(msgEl);

    this.container.appendChild(lineEl);
    this.linesCount++;

    if (this.linesCount > MAX_LOG_LINES) {
      if (this.container.firstChild) {
        this.container.removeChild(this.container.firstChild);
        this.linesCount--;
      }
    }

    if (this.autoScroll) {
      this.container.scrollTop = this.container.scrollHeight;
    }
  }

  clear() {
    this.container.innerHTML = '';
    this.linesCount = 0;
  }
}
