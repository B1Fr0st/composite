use std::time::Instant;

use newoverlay::{Overlay, OverlayConfig, imgui};

fn main() {
    let mut overlay = match Overlay::new(OverlayConfig {
        transparent: true,
        // The Discord compositor consumes back-buffer alpha. Alpha 1 here
        // would turn every otherwise-empty pixel into an opaque black quad.
        clear_color: [0.0, 0.0, 0.0, 0.0],
        ..OverlayConfig::default()
    }) {
        Ok(overlay) => overlay,
        Err(error) => {
            eprintln!("Failed to initialize overlay: {error}");
            return;
        }
    };

    // Transparent page regions reveal the draw-list primitives underneath.
    overlay.load_html(
        r#"<!doctype html>
        <style>
          html,body { width:100%; height:100%; margin:0; background:transparent; color:white;
                      font:18px system-ui; overflow:hidden; }
          .web-card { position:absolute; left:34%; top:24%; width:34%; padding:28px;
                      border:1px solid #ffffff55; border-radius:20px;
                      background:#111827d9; box-shadow:0 20px 80px #0008; }
          button { padding:10px 16px; color:white; background:#2563eb; border:0;
                   border-radius:8px; }
        </style>
        <section class="web-card">
          <h2>WebView layer</h2>
          <p>The animated primitives visible around this card are ImGui draw-list
             commands rendered beneath the captured WebView texture.</p>
          <button onclick="this.textContent='WebView clicked'">Web button</button>
        </section>"#,
    );

    let started = Instant::now();
    let mut show_controls = true;
    let smoke_frames = std::env::var("NEWOVERLAY_SMOKE_FRAMES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());
    let mut rendered_frames = 0_u64;
    while overlay.start_render() {
        let elapsed = started.elapsed().as_secs_f32();
        if let Err(error) = overlay.render(|ui, underlay| {
            let [width, height] = ui.io().display_size;
            let center = [width * 0.5, height * 0.5];
            let radius = 120.0 + elapsed.sin() * 24.0;

            underlay
                .add_circle(center, radius, [0.2, 0.8, 1.0, 0.9])
                .thickness(5.0)
                .build();
            underlay
                .add_line(
                    [40.0, height - 80.0],
                    [width - 40.0, height - 80.0],
                    [1.0, 0.25, 0.65, 0.9],
                )
                .thickness(8.0)
                .build();
            underlay.add_text(
                [40.0, 40.0],
                [0.85, 0.95, 1.0, 1.0],
                "ImGui underlay draw list",
            );

            // Regular ImGui windows are deliberately above the WebView layer.
            if show_controls {
                imgui::Window::new("Foreground ImGui")
                    .position([30.0, 100.0], imgui::Condition::FirstUseEver)
                    .size([280.0, 130.0], imgui::Condition::FirstUseEver)
                    .build(ui, || {
                        ui.text("This window is above WebView.");
                        if ui.button("Close controls") {
                            show_controls = false;
                        }
                    });
            }
        }) {
            eprintln!("Overlay render failed: {error}");
            break;
        }
        rendered_frames += 1;
        if smoke_frames.is_some_and(|limit| rendered_frames >= limit) {
            println!("rendered {rendered_frames} underlay + WebView + foreground frames");
            break;
        }
    }
}
