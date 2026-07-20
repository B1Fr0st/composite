use composite::{Overlay, OverlayConfig};

fn main() {
    let overlay = match Overlay::new(OverlayConfig {
        ipc_handler: Some(Box::new(|msg| println!("[mod-menu] {}", msg))),
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

  /* Three.js bg canvas */
  #bg { position: fixed; top: 0; left: 0; width: 100%; height: 100%;
        pointer-events: none; z-index: 0; }

  /* Menu panel */
  #menu {
    position: fixed; top: 50%; left: 50%; z-index: 10;
    transform: translate(-50%, -50%);
    width: 320px;
    background: rgba(8, 10, 18, 0.88);
    border: 1px solid rgba(80, 180, 255, 0.25);
    border-radius: 10px;
    box-shadow: 0 0 40px rgba(50, 140, 255, 0.15), 0 0 0 1px rgba(80,180,255,0.08);
    font-family: 'Segoe UI', system-ui, sans-serif;
    color: #d0e8ff;
    user-select: none;
    overflow: hidden;
  }

  /* Drag handle / title bar */
  #titlebar {
    display: flex; align-items: center; justify-content: space-between;
    padding: 10px 14px;
    background: rgba(30, 60, 120, 0.45);
    border-bottom: 1px solid rgba(80,180,255,0.15);
    cursor: move;
  }
  #titlebar .title { font-size: 13px; font-weight: 600; letter-spacing: 0.08em;
                     text-transform: uppercase; color: #7ec8ff; }
  #titlebar .subtitle { font-size: 10px; color: rgba(120,180,255,0.5);
                        letter-spacing: 0.12em; text-transform: uppercase; margin-top: 1px; }
  .close-btn {
    width: 18px; height: 18px; border-radius: 50%;
    background: rgba(255,80,80,0.3); border: 1px solid rgba(255,80,80,0.5);
    cursor: pointer; display: flex; align-items: center; justify-content: center;
    font-size: 10px; color: rgba(255,150,150,0.8); transition: background 0.2s;
  }
  .close-btn:hover { background: rgba(255,80,80,0.6); }

  /* Tab bar */
  #tabs {
    display: flex; border-bottom: 1px solid rgba(80,180,255,0.1);
    background: rgba(10,20,40,0.4);
  }
  .tab {
    flex: 1; padding: 8px 0; text-align: center; font-size: 11px;
    letter-spacing: 0.08em; text-transform: uppercase; color: rgba(120,180,255,0.45);
    cursor: pointer; transition: all 0.2s; border-bottom: 2px solid transparent;
  }
  .tab:hover { color: rgba(120,180,255,0.8); }
  .tab.active { color: #7ec8ff; border-bottom-color: #4090ff; }

  /* Tab content */
  .tab-content { display: none; padding: 12px 14px; }
  .tab-content.active { display: block; }

  /* Section label */
  .section {
    font-size: 9px; letter-spacing: 0.15em; text-transform: uppercase;
    color: rgba(100,160,255,0.4); margin: 10px 0 6px;
  }
  .section:first-child { margin-top: 0; }

  /* Checkbox row */
  .row {
    display: flex; align-items: center; justify-content: space-between;
    padding: 5px 0; border-bottom: 1px solid rgba(255,255,255,0.04);
  }
  .row:last-child { border-bottom: none; }
  .row label { font-size: 12px; color: #b0cce8; cursor: pointer; flex: 1; }
  .row .badge {
    font-size: 9px; padding: 1px 5px; border-radius: 3px; margin-right: 8px;
    background: rgba(255,180,0,0.15); color: rgba(255,200,80,0.7);
    border: 1px solid rgba(255,180,0,0.2); letter-spacing: 0.05em;
  }

  /* Toggle switch */
  .toggle { position: relative; width: 34px; height: 18px; flex-shrink: 0; }
  .toggle input { opacity: 0; width: 0; height: 0; }
  .toggle .track {
    position: absolute; inset: 0; border-radius: 9px; cursor: pointer;
    background: rgba(40,60,100,0.6); border: 1px solid rgba(80,140,255,0.2);
    transition: all 0.2s;
  }
  .toggle .track::after {
    content: ''; position: absolute; left: 2px; top: 2px;
    width: 12px; height: 12px; border-radius: 50%;
    background: rgba(120,160,220,0.5); transition: all 0.2s;
  }
  .toggle input:checked + .track {
    background: rgba(40,100,255,0.35); border-color: rgba(80,160,255,0.6);
    box-shadow: 0 0 8px rgba(60,120,255,0.3);
  }
  .toggle input:checked + .track::after {
    left: 18px; background: #5ab4ff;
    box-shadow: 0 0 6px rgba(90,180,255,0.8);
  }

  /* Slider row */
  .slider-row { padding: 6px 0; border-bottom: 1px solid rgba(255,255,255,0.04); }
  .slider-row:last-child { border-bottom: none; }
  .slider-header { display: flex; justify-content: space-between; margin-bottom: 5px; }
  .slider-header span { font-size: 12px; color: #b0cce8; }
  .slider-header .val {
    font-size: 11px; color: #5ab4ff; font-variant-numeric: tabular-nums;
    min-width: 32px; text-align: right;
  }
  input[type=range] {
    -webkit-appearance: none; appearance: none;
    width: 100%; height: 4px; border-radius: 2px; outline: none; cursor: pointer;
    background: rgba(40,80,160,0.4);
  }
  input[type=range]::-webkit-slider-thumb {
    -webkit-appearance: none; width: 12px; height: 12px; border-radius: 50%;
    background: #4a9eff; border: 2px solid rgba(160,210,255,0.6);
    box-shadow: 0 0 6px rgba(74,158,255,0.6); cursor: pointer;
  }

  /* Status bar */
  #statusbar {
    padding: 6px 14px; font-size: 10px; color: rgba(100,160,255,0.4);
    border-top: 1px solid rgba(80,180,255,0.08);
    display: flex; justify-content: space-between;
    background: rgba(6,10,22,0.5);
  }
  #statusbar .dot {
    display: inline-block; width: 6px; height: 6px; border-radius: 50%;
    background: #3f8; margin-right: 5px; vertical-align: middle;
    box-shadow: 0 0 6px #3f8;
  }
</style>
</head>
<body>

<canvas id="bg"></canvas>

<div id="menu">
  <div id="titlebar">
    <div>
      <div class="title">&#x25B6; Overlay Menu</div>
      <div class="subtitle">v1.0 &nbsp;·&nbsp; Injected</div>
    </div>
    <div class="close-btn" id="closeBtn">✕</div>
  </div>

  <div id="tabs">
    <div class="tab active" data-tab="combat">Combat</div>
    <div class="tab" data-tab="visual">Visual</div>
    <div class="tab" data-tab="misc">Misc</div>
  </div>

  <!-- COMBAT -->
  <div class="tab-content active" id="tab-combat">
    <div class="section">Aimbot</div>
    <div class="row">
      <label>Enable Aimbot</label>
      <span class="badge">HOT</span>
      <label class="toggle"><input type="checkbox" id="aimbot"><span class="track"></span></label>
    </div>
    <div class="row">
      <label>Silent Aim</label>
      <label class="toggle"><input type="checkbox" id="silent"><span class="track"></span></label>
    </div>
    <div class="slider-row">
      <div class="slider-header"><span>FOV Radius</span><span class="val" id="fov-val">80</span></div>
      <input type="range" id="fov" min="10" max="180" value="80">
    </div>
    <div class="slider-row">
      <div class="slider-header"><span>Smoothing</span><span class="val" id="smooth-val">0.40</span></div>
      <input type="range" id="smooth" min="0" max="100" value="40">
    </div>

    <div class="section">Combat</div>
    <div class="row">
      <label>Rapid Fire</label>
      <label class="toggle"><input type="checkbox" id="rapidfire"><span class="track"></span></label>
    </div>
    <div class="row">
      <label>No Recoil</label>
      <label class="toggle"><input type="checkbox" id="norecoil" checked><span class="track"></span></label>
    </div>
    <div class="slider-row">
      <div class="slider-header"><span>Trigger Delay (ms)</span><span class="val" id="delay-val">12</span></div>
      <input type="range" id="delay" min="0" max="100" value="12">
    </div>
  </div>

  <!-- VISUAL -->
  <div class="tab-content" id="tab-visual">
    <div class="section">ESP</div>
    <div class="row">
      <label>Player ESP</label>
      <label class="toggle"><input type="checkbox" id="esp" checked><span class="track"></span></label>
    </div>
    <div class="row">
      <label>Box ESP</label>
      <label class="toggle"><input type="checkbox" id="box"><span class="track"></span></label>
    </div>
    <div class="row">
      <label>Skeleton ESP</label>
      <label class="toggle"><input type="checkbox" id="skel"><span class="track"></span></label>
    </div>
    <div class="row">
      <label>Health Bar</label>
      <label class="toggle"><input type="checkbox" id="hpbar" checked><span class="track"></span></label>
    </div>
    <div class="slider-row">
      <div class="slider-header"><span>ESP Distance</span><span class="val" id="espdist-val">250</span></div>
      <input type="range" id="espdist" min="50" max="500" value="250">
    </div>

    <div class="section">Visuals</div>
    <div class="row">
      <label>Fullbright</label>
      <label class="toggle"><input type="checkbox" id="fullbright"><span class="track"></span></label>
    </div>
    <div class="row">
      <label>No Sky</label>
      <label class="toggle"><input type="checkbox" id="nosky"><span class="track"></span></label>
    </div>
    <div class="slider-row">
      <div class="slider-header"><span>FOV Override</span><span class="val" id="fovov-val">90</span></div>
      <input type="range" id="fovov" min="60" max="120" value="90">
    </div>
  </div>

  <!-- MISC -->
  <div class="tab-content" id="tab-misc">
    <div class="section">Movement</div>
    <div class="row">
      <label>Bunny Hop</label>
      <label class="toggle"><input type="checkbox" id="bhop"><span class="track"></span></label>
    </div>
    <div class="row">
      <label>Auto Strafe</label>
      <label class="toggle"><input type="checkbox" id="strafe"><span class="track"></span></label>
    </div>
    <div class="slider-row">
      <div class="slider-header"><span>Speed Multiplier</span><span class="val" id="speed-val">1.0x</span></div>
      <input type="range" id="speed" min="10" max="30" value="10">
    </div>

    <div class="section">Utility</div>
    <div class="row">
      <label>Panic Key (F10)</label>
      <label class="toggle"><input type="checkbox" id="panic" checked><span class="track"></span></label>
    </div>
    <div class="row">
      <label>Stream Proof</label>
      <span class="badge">NEW</span>
      <label class="toggle"><input type="checkbox" id="streamproof"><span class="track"></span></label>
    </div>
    <div class="slider-row">
      <div class="slider-header"><span>Menu Opacity</span><span class="val" id="opacity-val">88%</span></div>
      <input type="range" id="opacity" min="40" max="100" value="88">
    </div>
  </div>

  <div id="statusbar">
    <span><span class="dot"></span>Connected</span>
    <span id="fps-display">-- fps</span>
  </div>
</div>

<script type="importmap">
  { "imports": { "three": "https://cdn.jsdelivr.net/npm/three@0.163.0/build/three.module.js" } }
</script>
<script type="module">
import * as THREE from 'three';

// ─── Three.js background ───────────────────────────────────────────────────
const canvas = document.getElementById('bg');
const renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: true });
renderer.setPixelRatio(window.devicePixelRatio);
renderer.setClearColor(0x000000, 0);

const scene = new THREE.Scene();
const camera = new THREE.PerspectiveCamera(60, 1, 0.1, 100);
camera.position.z = 3;

// Particle field
const COUNT = 600;
const positions = new Float32Array(COUNT * 3);
const velocities = [];
for (let i = 0; i < COUNT; i++) {
  positions[i*3]   = (Math.random() - 0.5) * 20;
  positions[i*3+1] = (Math.random() - 0.5) * 20;
  positions[i*3+2] = (Math.random() - 0.5) * 10;
  velocities.push(
    (Math.random() - 0.5) * 0.004,
    (Math.random() - 0.5) * 0.004,
    0
  );
}
const geo = new THREE.BufferGeometry();
geo.setAttribute('position', new THREE.BufferAttribute(positions, 3));
const mat = new THREE.PointsMaterial({ color: 0x2060ff, size: 0.05, transparent: true, opacity: 0.6 });
const particles = new THREE.Points(geo, mat);
scene.add(particles);

// Grid plane
const gridGeo = new THREE.PlaneGeometry(30, 30, 30, 30);
const gridMat = new THREE.MeshBasicMaterial({
  color: 0x1040a0, wireframe: true, transparent: true, opacity: 0.07
});
const grid = new THREE.Mesh(gridGeo, gridMat);
grid.rotation.x = -Math.PI / 2.8;
grid.position.y = -4;
scene.add(grid);

function resize() {
  renderer.setSize(window.innerWidth, window.innerHeight);
  camera.aspect = window.innerWidth / window.innerHeight;
  camera.updateProjectionMatrix();
}
window.addEventListener('resize', resize);
resize();

let frame = 0;
let lastFpsTime = performance.now();
let fpsCount = 0;
const fpsEl = document.getElementById('fps-display');

(function animate() {
  requestAnimationFrame(animate);
  frame++;

  const pos = geo.attributes.position.array;
  for (let i = 0; i < COUNT; i++) {
    pos[i*3]   += velocities[i*3];
    pos[i*3+1] += velocities[i*3+1];
    if (Math.abs(pos[i*3])   > 10) velocities[i*3]   *= -1;
    if (Math.abs(pos[i*3+1]) > 10) velocities[i*3+1] *= -1;
  }
  geo.attributes.position.needsUpdate = true;
  particles.rotation.z += 0.0003;
  grid.position.z = Math.sin(frame * 0.005) * 0.5;

  renderer.render(scene, camera);

  // FPS counter
  fpsCount++;
  const now = performance.now();
  if (now - lastFpsTime >= 1000) {
    fpsEl.textContent = fpsCount + ' fps';
    fpsCount = 0;
    lastFpsTime = now;
  }
})();

// ─── Menu drag ─────────────────────────────────────────────────────────────
const menu = document.getElementById('menu');
const titlebar = document.getElementById('titlebar');
let dragging = false, ox = 0, oy = 0;
titlebar.addEventListener('mousedown', e => {
  dragging = true;
  const r = menu.getBoundingClientRect();
  ox = e.clientX - r.left; oy = e.clientY - r.top;
  menu.style.transform = 'none';
  menu.style.left = r.left + 'px';
  menu.style.top  = r.top  + 'px';
  e.preventDefault();
});
document.addEventListener('mousemove', e => {
  if (!dragging) return;
  menu.style.left = (e.clientX - ox) + 'px';
  menu.style.top  = (e.clientY - oy) + 'px';
});
document.addEventListener('mouseup', () => { dragging = false; });

// ─── Close button ──────────────────────────────────────────────────────────
document.getElementById('closeBtn').addEventListener('click', () => {
  menu.style.display = 'none';
  window.ipc.postMessage('menu_closed');
});

// ─── Tabs ──────────────────────────────────────────────────────────────────
document.querySelectorAll('.tab').forEach(tab => {
  tab.addEventListener('click', () => {
    document.querySelectorAll('.tab').forEach(t => t.classList.remove('active'));
    document.querySelectorAll('.tab-content').forEach(c => c.classList.remove('active'));
    tab.classList.add('active');
    document.getElementById('tab-' + tab.dataset.tab).classList.add('active');
  });
});

// ─── Toggles ───────────────────────────────────────────────────────────────
document.querySelectorAll('input[type=checkbox]').forEach(cb => {
  cb.addEventListener('change', () => {
    window.ipc.postMessage(JSON.stringify({ type: 'toggle', id: cb.id, value: cb.checked }));
  });
});

// ─── Sliders ───────────────────────────────────────────────────────────────
const sliderMap = {
  fov:     { el: 'fov-val',      fmt: v => v },
  smooth:  { el: 'smooth-val',   fmt: v => (v/100).toFixed(2) },
  delay:   { el: 'delay-val',    fmt: v => v },
  espdist: { el: 'espdist-val',  fmt: v => v },
  fovov:   { el: 'fovov-val',    fmt: v => v },
  speed:   { el: 'speed-val',    fmt: v => (v/10).toFixed(1)+'x' },
  opacity: { el: 'opacity-val',  fmt: v => v+'%',
             cb: v => { document.getElementById('menu').style.background =
               `rgba(8,10,18,${v/100})`; } },
};

Object.entries(sliderMap).forEach(([id, cfg]) => {
  const input = document.getElementById(id);
  if (!input) return;
  input.addEventListener('input', () => {
    const v = parseInt(input.value);
    document.getElementById(cfg.el).textContent = cfg.fmt(v);
    if (cfg.cb) cfg.cb(v);
    window.ipc.postMessage(JSON.stringify({ type: 'slider', id, value: v }));
  });
});
</script>
</body>
</html>"#);

    println!("Mod menu overlay initialized.");
    overlay.run();
    println!("Overlay shutting down.");
}
