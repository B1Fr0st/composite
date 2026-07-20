use newoverlay::{Overlay, OverlayConfig};

fn main() {
    let overlay = match Overlay::new(OverlayConfig {
        ipc_handler: Some(Box::new(|msg| println!("[ipc] {}", msg))),
        ..OverlayConfig::default()
    }) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Failed: {}", e);
            return;
        }
    };

    overlay.load_html(r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  html, body { width: 100%; height: 100%; background: #1a1a2e; overflow: hidden;
               font-family: monospace; color: white; }
  #panel { position: fixed; top: 40px; left: 40px; width: 400px;
           background: rgba(0,0,0,0.8); border: 1px solid #444;
           border-radius: 8px; padding: 20px; }
  h2 { font-size: 14px; color: #7ec8ff; margin-bottom: 16px; }
  .row { margin-bottom: 14px; }
  .label { font-size: 11px; color: #aaa; display: flex; justify-content: space-between; margin-bottom: 4px; }
  input[type=range] { width: 100%; height: 6px; cursor: pointer; -webkit-appearance: none; appearance: none;
                      background: #333; border-radius: 3px; outline: none; }
  input[type=range]::-webkit-slider-thumb { -webkit-appearance: none; width: 16px; height: 16px;
    border-radius: 50%; background: #4a9eff; border: 2px solid #adf; cursor: pointer; }
  #log { margin-top: 16px; border-top: 1px solid #333; padding-top: 10px;
         font-size: 10px; color: #0f0; max-height: 300px; overflow-y: auto; }
  #log div { padding: 1px 0; border-bottom: 1px solid rgba(255,255,255,0.05); }
  #captured { margin-top: 8px; font-size: 11px; color: #ff0; }
</style>
</head>
<body>
<div id="panel">
  <h2>Slider Debug</h2>

  <div class="row">
    <div class="label"><span>Slider A</span><span id="a-val">50</span></div>
    <input type="range" id="a" min="0" max="100" value="50">
  </div>
  <div class="row">
    <div class="label"><span>Slider B</span><span id="b-val">25</span></div>
    <input type="range" id="b" min="0" max="100" value="25">
  </div>

  <div id="captured">Captured: none</div>

  <div id="log"></div>
</div>

<script>
  // ── value display ─────────────────────────────────────────────────────────
  document.getElementById('a').addEventListener('input', e => {
    document.getElementById('a-val').textContent = e.target.value;
    log('slider A input → ' + e.target.value);
  });
  document.getElementById('b').addEventListener('input', e => {
    document.getElementById('b-val').textContent = e.target.value;
    log('slider B input → ' + e.target.value);
  });

  // ── log helper ────────────────────────────────────────────────────────────
  const logEl = document.getElementById('log');
  function log(msg) {
    const d = document.createElement('div');
    d.textContent = new Date().toISOString().slice(11,23) + '  ' + msg;
    logEl.prepend(d);
    if (logEl.children.length > 60) logEl.lastChild.remove();
    window.ipc.postMessage(msg);
  }

  // ── track every synthetic event hitting the document ─────────────────────
  ['mousedown','mousemove','mouseup','click','pointerdown','pointermove','pointerup'].forEach(type => {
    document.addEventListener(type, e => {
      if (type === 'mousemove' && e.buttons === 0) return; // skip hover noise
      const tgt = e.target ? (e.target.tagName + (e.target.id ? '#'+e.target.id : '')) : '?';
      const msg = `${type}  btn=${e.button} buttons=${e.buttons}  (${e.clientX|0},${e.clientY|0})  tgt=${tgt}`;
      if (type !== 'mousemove') log(msg);
    }, true);
  });

  // ── watch __capturedEl changes ────────────────────────────────────────────
  const capEl = document.getElementById('captured');
  setInterval(() => {
    const c = window.__capturedEl;
    const name = c ? (c.tagName + (c.id ? '#'+c.id : '')) : 'none';
    capEl.textContent = 'Captured: ' + name + '  |  buttons_mask: ' + (window.__lastButtonsMask ?? '?');
  }, 50);
</script>
</body>
</html>"#);

    println!("Slider debug overlay running. Watch the log panel and terminal.");
    overlay.run();
}
