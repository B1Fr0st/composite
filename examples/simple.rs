use composite::{Overlay, OverlayConfig};

fn main() {
    let overlay = match Overlay::new(OverlayConfig {
        ipc_handler: Some(Box::new(|msg| println!("[ipc] {}", msg))),
        ..OverlayConfig::default()
    }) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Failed to initialize overlay: {}", e);
            return;
        }
    };

    overlay.load_html(r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  html, body { width: 100%; height: 100%; background: transparent; overflow: hidden; }
  canvas { position: absolute; top: 0; left: 0; pointer-events: none; }
  #ui { position: absolute; top: 20px; left: 20px; color: white; font-family: monospace;
        background: rgba(0,0,0,0.7); padding: 12px 16px; border-radius: 8px; min-width: 320px; }
  #ui div { margin: 2px 0; }
  #log { margin-top: 8px; font-size: 11px; opacity: 0.8; max-height: 160px; overflow-y: auto; }
  #log div { border-bottom: 1px solid rgba(255,255,255,0.1); padding: 1px 0; }
  button { margin-top: 8px; padding: 6px 14px; cursor: pointer;
           background: rgba(255,100,100,0.4); border: 1px solid rgba(255,100,100,0.8);
           color: white; border-radius: 4px; font-size: 13px; }
</style>
</head>
<body>
<canvas id="c"></canvas>
<div id="ui">
  <div id="mouse">Mouse: (0, 0)</div>
  <div id="buttons">Buttons: L=0 M=0 R=0</div>
  <div id="last_event">Last event: —</div>
  <div id="hit_test">Hit element: —</div>
  <button id="btn">Click me</button>
  <div id="log"></div>
</div>

<script>
  // --- canvas drawing ---
  const canvas = document.getElementById('c');
  const ctx = canvas.getContext('2d');
  function resize() {
    canvas.width = window.innerWidth;
    canvas.height = window.innerHeight;
    draw();
  }
  function draw() {
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.strokeStyle = 'red'; ctx.lineWidth = 2;
    ctx.beginPath(); ctx.moveTo(100, 100); ctx.lineTo(300, 100); ctx.stroke();
    ctx.fillStyle = 'rgba(0,255,0,0.5)';
    ctx.fillRect(100, 120, 200, 100);
    ctx.strokeStyle = 'lime'; ctx.strokeRect(100, 120, 200, 100);
    ctx.beginPath(); ctx.arc(450, 170, 50, 0, Math.PI*2);
    ctx.fillStyle = 'rgba(255,255,0,0.5)'; ctx.fill();
    ctx.strokeStyle = 'yellow'; ctx.stroke();
    ctx.fillStyle = 'white'; ctx.font = '16px monospace';
    ctx.fillText('Direct canvas rendering!', 100, 240);
  }
  window.addEventListener('resize', resize);
  resize();

  // --- logging ---
  const logEl = document.getElementById('log');
  function log(msg) {
    const d = document.createElement('div');
    d.textContent = new Date().toLocaleTimeString('en',{hour12:false}) + '  ' + msg;
    logEl.prepend(d);
    if (logEl.children.length > 30) logEl.lastChild.remove();
  }

  // --- debug: track every mouse event on document ---
  ['mousemove','mousedown','mouseup','click','pointerdown','pointerup'].forEach(type => {
    document.addEventListener(type, e => {
      if (type === 'mousemove') {
        document.getElementById('mouse').textContent =
          `Mouse: (${e.clientX}, ${e.clientY})`;
        document.getElementById('buttons').textContent =
          `Buttons: L=${e.buttons&1} M=${(e.buttons>>2)&1} R=${(e.buttons>>1)&1}  .buttons=${e.buttons}`;
        // hit-test: what element is actually under the cursor?
        const hit = document.elementFromPoint(e.clientX, e.clientY);
        document.getElementById('hit_test').textContent =
          'Hit: ' + (hit ? hit.tagName + (hit.id ? '#'+hit.id : '') : 'null');
      } else {
        const msg = `${type}  btn=${e.button}  buttons=${e.buttons}  (${e.clientX},${e.clientY})  tgt=${e.target?.tagName||'?'}${e.target?.id?'#'+e.target.id:''}`;
        document.getElementById('last_event').textContent = 'Last: ' + msg;
        log(msg);
      }
    }, true); // capture phase — fires regardless of target
  });

  // --- button handler (both onclick and addEventListener) ---
  const btn = document.getElementById('btn');
  btn.addEventListener('click', () => {
    log('>>> btn click via addEventListener');
    window.ipc.postMessage('button_clicked');
  });
  btn.addEventListener('mousedown', () => log('>>> btn mousedown'));
  btn.addEventListener('mouseup',   () => log('>>> btn mouseup'));
  btn.onclick = () => log('>>> btn onclick');
</script>
</body>
</html>"#);

    println!("Overlay initialized.");
    overlay.run();
    println!("Overlay shutting down.");
}
