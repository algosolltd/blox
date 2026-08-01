'use strict';
// Auto-dismissing toast notifications.
//
//   import { Toaster } from './components/toaster.js';
//   const toaster = new Toaster(rootEl);
//   toaster.show('warn', 'order rejected');

export class Toaster {
  // Three at a time: the stack sits over a panel, and a burst that fills the
  // screen hides the thing the notification is telling you about.
  constructor(root, { max = 3, durationMs = 4200 } = {}) {
    this.root = root;
    this.max = max;
    this.durationMs = durationMs;
  }

  show(level, text) {
    const t = document.createElement('div');
    t.className = 'toast ' + level;
    t.textContent = text;
    this.root.appendChild(t);
    while (this.root.children.length > this.max) this.root.firstChild.remove();
    setTimeout(() => {
      t.classList.add('out');
      setTimeout(() => t.remove(), 300);
    }, this.durationMs);
  }
}
