use composite::{Overlay, OverlayConfig};

fn main() {
    // Use new_with_ipc_queue so the IPC closure in run_with_ipc_mut can
    // reposition/resize the webview in response to drag/resize messages from JS.
    let overlay = match Overlay::new_with_ipc_queue(OverlayConfig {
        transparent: false,
        ipc_handler: None,
        ..OverlayConfig::default()
    }) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Failed to initialize overlay: {}", e);
            return;
        }
    };

    // Start at 720×480 in the top-left of Discord's client area.
    {
        use wry::dpi::{PhysicalPosition, PhysicalSize};
        let _ = overlay.webview().set_bounds(wry::Rect {
            position: PhysicalPosition::new(60, 60).into(),
            size: PhysicalSize::new(720u32, 480u32).into(),
        });
    }

    overlay.load_url("https://www.youtube.com");

    // Inject a draggable/resizable chrome bar on top of YouTube.
    // We poll for document.body being ready rather than DOMContentLoaded because
    // evaluate_script fires before YouTube's SPA has fully rendered.
    overlay.eval(r#"
(function waitAndInstall() {
    if (!document.body) { setTimeout(waitAndInstall, 50); return; }
    if (document.getElementById('__overlay_bar')) return;

    // ── Title bar ────────────────────────────────────────────────────────
    const bar = document.createElement('div');
    bar.id = '__overlay_bar';
    Object.assign(bar.style, {
        position:'fixed', top:'0', left:'0', right:'0', height:'28px',
        background:'#1a1a1a', borderBottom:'1px solid rgba(255,255,255,0.1)',
        display:'flex', alignItems:'center', justifyContent:'space-between',
        padding:'0 10px', zIndex:'2147483647', cursor:'move', userSelect:'none',
        fontFamily:'Segoe UI,sans-serif', boxSizing:'border-box',
    });

    const label = document.createElement('span');
    label.textContent = '▶ YouTube';
    Object.assign(label.style, { fontSize:'12px', color:'rgba(255,255,255,0.55)', letterSpacing:'.05em' });
    bar.appendChild(label);

    const btnGroup = document.createElement('div');
    const btnStyle = { display:'inline-block', width:'13px', height:'13px',
        borderRadius:'50%', marginLeft:'6px', cursor:'pointer', border:'none', verticalAlign:'middle' };

    function makeBtn(color, title) {
        const b = document.createElement('button');
        Object.assign(b.style, { ...btnStyle, background: color });
        b.title = title;
        return b;
    }
    const btnMin   = makeBtn('#febc2e', 'Minimise');
    const btnReset = makeBtn('#28c840', 'Reset size');
    const btnClose = makeBtn('#ff5f57', 'Close');
    [btnMin, btnReset, btnClose].forEach(b => btnGroup.appendChild(b));
    bar.appendChild(btnGroup);
    document.body.prepend(bar);
    document.body.style.paddingTop = '28px';

    // ── Resize grip ───────────────────────────────────────────────────────
    const grip = document.createElement('div');
    grip.id = '__overlay_grip';
    Object.assign(grip.style, {
        position:'fixed', right:'0', bottom:'0', width:'20px', height:'20px',
        cursor:'se-resize', zIndex:'2147483647',
    });
    grip.innerHTML = `<svg width="20" height="20" style="position:absolute;right:2px;bottom:2px">
      <line x1="6"  y1="18" x2="18" y2="6"  stroke="rgba(255,255,255,0.2)" stroke-width="1.5"/>
      <line x1="10" y1="18" x2="18" y2="10" stroke="rgba(255,255,255,0.2)" stroke-width="1.5"/>
      <line x1="14" y1="18" x2="18" y2="14" stroke="rgba(255,255,255,0.2)" stroke-width="1.5"/>
    </svg>`;
    document.body.appendChild(grip);

    // ── Drag ─────────────────────────────────────────────────────────────
    let drag = null;
    bar.addEventListener('mousedown', e => {
        if (e.target !== bar && e.target !== label) return;
        drag = { sx: e.screenX, sy: e.screenY };
        e.preventDefault();
    });
    document.addEventListener('mousemove', e => {
        if (!drag) return;
        window.ipc.postMessage(JSON.stringify({
            type:'move', dx: e.screenX - drag.sx, dy: e.screenY - drag.sy
        }));
        drag.sx = e.screenX; drag.sy = e.screenY;
    });
    document.addEventListener('mouseup', () => { drag = null; });

    // ── Resize ────────────────────────────────────────────────────────────
    let resz = null;
    grip.addEventListener('mousedown', e => {
        resz = { sx: e.screenX, sy: e.screenY };
        e.preventDefault(); e.stopPropagation();
    });
    document.addEventListener('mousemove', e => {
        if (!resz) return;
        window.ipc.postMessage(JSON.stringify({
            type:'resize', dx: e.screenX - resz.sx, dy: e.screenY - resz.sy
        }));
        resz.sx = e.screenX; resz.sy = e.screenY;
    });
    document.addEventListener('mouseup', () => { resz = null; });

    // ── Buttons ───────────────────────────────────────────────────────────
    let minimised = false;
    btnMin.addEventListener('click', () => {
        minimised = !minimised;
        window.ipc.postMessage(JSON.stringify({ type: minimised ? 'minimise' : 'restore' }));
    });
    btnReset.addEventListener('click', () => {
        minimised = false;
        window.ipc.postMessage(JSON.stringify({ type:'reset' }));
    });
    btnClose.addEventListener('click', () => {
        window.ipc.postMessage(JSON.stringify({ type:'close' }));
    });
})();
"#);

    println!("YouTube overlay — drag title bar to move, grip to resize.");

    overlay.run_with_ipc_mut(|msg, webview, bounds| {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&msg) else {
            return;
        };

        match val["type"].as_str() {
            Some("move") => {
                let dx = val["dx"].as_i64().unwrap_or(0) as i32;
                let dy = val["dy"].as_i64().unwrap_or(0) as i32;
                let pos: wry::dpi::PhysicalPosition<i32> = bounds.position.to_physical(1.0);
                let new_pos =
                    wry::dpi::PhysicalPosition::new((pos.x + dx).max(0), (pos.y + dy).max(0));
                bounds.position = new_pos.into();
                let _ = webview.set_bounds(*bounds);
            }
            Some("resize") => {
                let dx = val["dx"].as_i64().unwrap_or(0) as i32;
                let dy = val["dy"].as_i64().unwrap_or(0) as i32;
                let sz: wry::dpi::PhysicalSize<u32> = bounds.size.to_physical(1.0);
                let new_sz = wry::dpi::PhysicalSize::new(
                    ((sz.width as i32 + dx).max(320)) as u32,
                    ((sz.height as i32 + dy).max(220)) as u32,
                );
                bounds.size = new_sz.into();
                let _ = webview.set_bounds(*bounds);
            }
            Some("minimise") => {
                let pos: wry::dpi::PhysicalPosition<i32> = bounds.position.to_physical(1.0);
                let _ = webview.set_bounds(wry::Rect {
                    position: pos.into(),
                    size: wry::dpi::PhysicalSize::new(
                        bounds.size.to_physical::<u32>(1.0).width,
                        28u32,
                    )
                    .into(),
                });
            }
            Some("restore") | Some("reset") => {
                let pos: wry::dpi::PhysicalPosition<i32> = bounds.position.to_physical(1.0);
                let new_sz = wry::dpi::PhysicalSize::new(720u32, 480u32);
                bounds.size = new_sz.into();
                let _ = webview.set_bounds(wry::Rect {
                    position: pos.into(),
                    size: new_sz.into(),
                });
            }
            Some("close") => {
                // Hide the webview by shrinking to zero — loop will exit on Discord close.
                let _ = webview.set_bounds(wry::Rect {
                    position: wry::dpi::PhysicalPosition::new(0, 0).into(),
                    size: wry::dpi::PhysicalSize::new(0u32, 0u32).into(),
                });
            }
            _ => {
                println!("[youtube] {}", msg);
            }
        }
    });

    println!("Overlay shutting down.");
}
