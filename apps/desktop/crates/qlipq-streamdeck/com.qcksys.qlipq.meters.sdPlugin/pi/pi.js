// Shared property inspector wiring: hands the action's settings to the page when it opens and
// when they change, and saves whatever the page passes to `pi.save`.
const pi = {
  socket: null,
  context: null,
  settings: {},
  /** Called with the action's settings. */
  onSettings: () => {},
  onConnected: () => {},
  /** Called with each payload the plugin sends with sendToPropertyInspector. */
  onPluginMessage: () => {},
  save(settings) {
    this.settings = settings;
    this.socket?.send(JSON.stringify({ event: "setSettings", context: this.context, payload: settings }));
  },
  /** Reads a number field, falling back when it's empty or not a number. */
  number(element, fallback) {
    const value = Number.parseFloat(element.value);
    return Number.isFinite(value) ? value : fallback;
  },
};

window.connectElgatoStreamDeckSocket = (port, uuid, registerEvent, info, actionInfo) => {
  pi.context = uuid;
  const action = JSON.parse(actionInfo);
  pi.action = action.action;
  pi.settings = action.payload.settings ?? {};
  pi.onSettings(pi.settings);
  pi.socket = new WebSocket(`ws://127.0.0.1:${port}`);
  pi.socket.onopen = () => {
    pi.socket.send(JSON.stringify({ event: registerEvent, uuid }));
    pi.onConnected();
  };
  pi.socket.onmessage = (message) => {
    const data = JSON.parse(message.data);
    if (data.event === "didReceiveSettings") {
      pi.settings = data.payload.settings ?? {};
      pi.onSettings(pi.settings);
    }
    if (data.event === "sendToPropertyInspector") pi.onPluginMessage(data.payload);
  };
};
