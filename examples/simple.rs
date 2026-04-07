use newoverlay::{Overlay, OverlayConfig};

fn main() {
    let overlay = match Overlay::new(OverlayConfig {
        ipc_handler: Some(Box::new(|msg| println!("[ipc] {}", msg))),
        ..OverlayConfig::default()
    }) {
        Ok(o) => o,
        Err(e) => { eprintln!("Failed to initialize overlay: {}", e); return; }
    };

    overlay.load_html(r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  html, body { width: 100%; height: 100%; background: transparent; overflow: hidden; }
  canvas { position: absolute; top: 0; left: 0; }
  #ui { position: absolute; top: 20px; left: 20px; color: white; font-family: sans-serif;
        background: rgba(0,0,0,0.5); padding: 12px 16px; border-radius: 8px; }
  button { margin-top: 8px; padding: 4px 10px; cursor: pointer; }
</style>
</head>
<body>
<canvas id="c"></canvas>
<div id="ui">
  <div>Overlay running!</div>
  <div id="mouse">Mouse: (0, 0)</div>
  <button onclick="window.ipc.postMessage('button_clicked')">Click me</button>
</div>
<script>
  const canvas = document.getElementById('c');
  const ctx = canvas.getContext('2d');

  function resize() {
    canvas.width = window.innerWidth;
    canvas.height = window.innerHeight;
    draw();
  }

  function draw() {
    ctx.clearRect(0, 0, canvas.width, canvas.height);

    // Red line
    ctx.strokeStyle = 'red'; ctx.lineWidth = 2;
    ctx.beginPath(); ctx.moveTo(100, 100); ctx.lineTo(300, 100); ctx.stroke();

    // Green filled rect
    ctx.fillStyle = 'rgba(0,255,0,0.5)';
    ctx.fillRect(100, 120, 200, 100);
    ctx.strokeStyle = 'rgba(0,255,0,1)'; ctx.lineWidth = 2;
    ctx.strokeRect(100, 120, 200, 100);

    // Yellow circle
    ctx.beginPath(); ctx.arc(450, 170, 50, 0, Math.PI * 2);
    ctx.fillStyle = 'rgba(255,255,0,0.5)'; ctx.fill();
    ctx.strokeStyle = 'yellow'; ctx.lineWidth = 2; ctx.stroke();

    // Text
    ctx.fillStyle = 'white'; ctx.font = '16px sans-serif';
    ctx.fillText('Direct canvas rendering!', 100, 240);
  }

  window.addEventListener('resize', resize);
  window.addEventListener('mousemove', (e) => {
    document.getElementById('mouse').textContent = `Mouse: (${e.clientX}, ${e.clientY})`;
  });
  resize();
</script>
</body>
</html>"#);

    println!("Overlay initialized.");
    overlay.run();
    println!("Overlay shutting down.");
}
