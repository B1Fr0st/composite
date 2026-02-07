use imgui::*;
use newoverlay::Overlay;

fn main() {
    let mut overlay = match Overlay::new() {
        Some(o) => o,
        None => {
            eprintln!("Failed to initialize overlay");
            return;
        }
    };

    println!("Overlay initialized successfully!");

    // Main render loop
    loop {
        if !overlay.start_render() {
            break;
        }

        // Render UI
        overlay.render(|ui| {
            Window::new("Overlay")
                .size([300.0, 250.0], Condition::FirstUseEver)
                .build(ui, || {
                    ui.text("Hello from ImGui!");
                    if ui.button("Click me") {
                        println!("Button clicked!");
                    }

                    ui.separator();
                    ui.text("Mouse Debug:");
                    ui.text(format!("Position: ({:.1}, {:.1})",
                        ui.io().mouse_pos[0],
                        ui.io().mouse_pos[1]));
                    ui.text(format!("Left: {} | Right: {} | Middle: {}",
                        ui.io().mouse_down[0],
                        ui.io().mouse_down[1],
                        ui.io().mouse_down[2]));
                    ui.text(format!("Wheel: {:.2} | WheelH: {:.2}",
                        ui.io().mouse_wheel,
                        ui.io().mouse_wheel_h));
                });
        });
    }

    println!("Overlay shutting down");
}
