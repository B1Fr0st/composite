# composite

`composite` is a Windows rendering library derived from
[`NoctisMenu/newoverlay`](https://github.com/NoctisMenu/newoverlay). It captures
a Wry/WebView2 surface into a D3D11 texture, composes it with ImGui, and presents
the final shared texture through Discord's overlay window.

Layer order is deterministic:

1. caller primitives submitted through the underlay draw list;
2. the captured WebView texture;
3. ordinary ImGui windows and foreground primitives.

Run the combined-layer example while Discord is running:

```powershell
cargo run --example underlay
```

JavaScript-to-Rust IPC remains available through `window.ipc.postMessage` and
`OverlayConfig::ipc_handler`, or through `Overlay::new_with_ipc_queue` together
with `run_with_ipc`/`run_with_ipc_mut`.
