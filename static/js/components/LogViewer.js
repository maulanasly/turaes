import { html } from "../lib/html.js";
import { useEffect, useRef, useState } from "preact/hooks";

const MAX_LINES = 500;

export function LogViewer({ appId, height = 300 }) {
  const [lines, setLines] = useState([]);
  const [status, setStatus] = useState("connecting…");
  const [autoscroll, setAutoscroll] = useState(true);
  const preRef = useRef(null);

  useEffect(() => {
    let closed = false;
    let ws;
    let retry;

    const connect = () => {
      const proto = location.protocol === "https:" ? "wss" : "ws";
      ws = new WebSocket(`${proto}://${location.host}/api/v1/apps/${appId}/logs`);
      setStatus("connecting…");
      ws.onopen = () => setStatus("live");
      ws.onmessage = (ev) => {
        setLines((prev) => {
          const next = prev.concat(String(ev.data).split("\n"));
          return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next;
        });
      };
      ws.onerror = () => setStatus("error");
      ws.onclose = () => {
        if (closed) return;
        setStatus("reconnecting…");
        retry = setTimeout(connect, 2000);
      };
    };
    connect();

    return () => {
      closed = true;
      clearTimeout(retry);
      if (ws) ws.close();
    };
  }, [appId]);

  useEffect(() => {
    if (autoscroll && preRef.current) preRef.current.scrollTop = preRef.current.scrollHeight;
  }, [lines, autoscroll]);

  const download = () => {
    const blob = new Blob([lines.join("\n")], { type: "text/plain" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `${appId}.log`;
    a.click();
    URL.revokeObjectURL(a.href);
  };

  return html`
    <div>
      <div class="panel-head">
        <h2>Logs <span class="muted">· ${status}</span></h2>
        <div class="controls">
          <label class="inline"><input type="checkbox" checked=${autoscroll}
            onChange=${(e) => setAutoscroll(e.target.checked)} /> autoscroll</label>
          <button class="btn small ghost" onClick=${() => setLines([])}>Clear</button>
          <button class="btn small ghost" onClick=${download}>Download</button>
        </div>
      </div>
      <pre class="log" style=${`max-height:${height}px`} ref=${preRef}>${lines.join("\n")}</pre>
    </div>`;
}
