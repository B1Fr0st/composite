use newoverlay::{Overlay, OverlayConfig};

fn main() {
    let overlay = match Overlay::new(OverlayConfig {
        ipc_handler: Some(Box::new(|msg| match msg.as_str() {
            "reset_font" => println!("[ipc] Font reset requested"),
            other => println!("[ipc] {}", other),
        })),
        ..OverlayConfig::default()
    }) {
        Ok(o) => o,
        Err(e) => { eprintln!("Failed to initialize overlay: {}", e); return; }
    };

    // Replace the src value with a base64-encoded data URI to embed a custom TTF/OTF.
    overlay.load_html(r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  /* @font-face { font-family: 'MyFont'; src: url('data:font/ttf;base64,...'); } */
  * { margin: 0; padding: 0; box-sizing: border-box; }
  html, body { width: 100%; height: 100%; background: transparent; overflow: hidden;
               font-family: 'Segoe UI', sans-serif; color: white; }
  .panel { position: absolute; top: 20px; left: 20px;
           background: rgba(0,0,0,0.6); padding: 20px 24px; border-radius: 10px; min-width: 340px; }
  .large { font-size: 20px; }
  .small { font-size: 14px; }
  .mono  { font-family: 'Consolas', monospace; font-size: 16px; }
  hr { border: none; border-top: 1px solid rgba(255,255,255,0.2); margin: 10px 0; }
  button { margin-top: 12px; padding: 6px 14px; cursor: pointer;
           background: rgba(255,255,255,0.15); border: 1px solid rgba(255,255,255,0.3);
           color: white; border-radius: 4px; }
</style>
</head>
<body>
<div class="panel">
  <h2 id="title">Custom Font Example</h2>
  <p>Default font.</p><hr>
  <p class="large">Large (20px)</p><hr>
  <p class="small">Small (14px)</p><hr>
  <p class="mono">Monospace (16px)</p><hr>
  <button onclick="window.ipc.postMessage('reset_font')">Reset font</button>
</div>
</body>
</html>"#);

    // Demonstrate eval: change the title after 3 seconds from a background thread.
    let webview_ref = overlay.webview() as *const _ as usize;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        // NOTE: eval from another thread is safe via the webview's internal queue.
        // Cast back — the webview lives for the duration of the program.
        let wv = unsafe { &*(webview_ref as *const wry::WebView) };
        let _ = wv.evaluate_script(
            "document.getElementById('title').textContent = 'Injected after 3s!'"
        );
    });

    println!("Overlay initialized.");
    overlay.run();
    println!("Overlay shutting down.");
}
