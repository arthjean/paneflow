(() => {
  const binding = Object.getOwnPropertyDescriptor(window, '__paneflowInspectorSend');
  const send = (message) => {
    const target = binding ? binding.value : window.__paneflowInspectorSend;
    if (typeof target === 'function') target(message);
  };
  const drawMenusInPage = () => {
    try {
      window.InspectorFrontendAPI?.setUseSoftMenu(true);
    } catch {}
  };
  for (const type of ['pointerdown', 'mousedown', 'contextmenu', 'keydown']) {
    window.addEventListener(type, drawMenusInPage, true);
  }
  const sockets = new Set();
  const fire = (socket, type, event) => {
    const handler = socket['on' + type];
    if (typeof handler === 'function') handler.call(socket, event);
    for (const listener of socket.listeners[type] || []) listener.call(socket, event);
  };
  class InspectorSocket {
    constructor(url) {
      this.url = String(url);
      this.readyState = 0;
      this.listeners = {};
      this.onopen = this.onmessage = this.onclose = this.onerror = null;
      sockets.add(this);
      queueMicrotask(() => {
        this.readyState = 1;
        fire(this, 'open', { type: 'open' });
      });
    }
    send(message) {
      if (this.readyState === 1) send(String(message));
    }
    close() {
      if (this.readyState === 3) return;
      this.readyState = 3;
      sockets.delete(this);
      fire(this, 'close', { type: 'close', code: 1000, reason: '', wasClean: true });
    }
    addEventListener(type, listener) {
      (this.listeners[type] ||= []).push(listener);
    }
    removeEventListener(type, listener) {
      this.listeners[type] = (this.listeners[type] || []).filter((each) => each !== listener);
    }
  }
  InspectorSocket.CONNECTING = 0;
  InspectorSocket.OPEN = 1;
  InspectorSocket.CLOSING = 2;
  InspectorSocket.CLOSED = 3;
  Object.defineProperty(window, 'WebSocket', { value: InspectorSocket, configurable: false, writable: false });
  Object.defineProperty(window, '__paneflowInspectorReceive', {
    value: (data) => {
      for (const socket of sockets) fire(socket, 'message', { type: 'message', data });
    },
    configurable: false,
    writable: false,
  });
})();
