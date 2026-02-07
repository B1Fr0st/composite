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
            // Get background draw list (draws behind windows)
            let draw_list = ui.get_background_draw_list();

            // Draw a line
            draw_list
                .add_line([100.0, 100.0], [300.0, 100.0], [1.0, 0.0, 0.0, 1.0])
                .thickness(2.0)
                .build();

            // Draw a rectangle (filled)
            draw_list.add_rect(
                [100.0, 120.0],
                [300.0, 220.0],
                [0.0, 1.0, 0.0, 0.5] // Green with 50% alpha
            )
            .filled(true)
            .build();

            // Draw a rectangle (outline)
            draw_list.add_rect(
                [100.0, 120.0],
                [300.0, 220.0],
                [0.0, 1.0, 0.0, 1.0]
            )
            .thickness(2.0)
            .build();

            // Draw a circle (filled)
            draw_list.add_circle(
                [450.0, 170.0],
                50.0,
                [1.0, 1.0, 0.0, 0.5] // Yellow with 50% alpha
            )
            .filled(true)
            .build();

            // Draw a circle (outline)
            draw_list.add_circle(
                [450.0, 170.0],
                50.0,
                [1.0, 1.0, 0.0, 1.0]
            )
            .thickness(2.0)
            .build();

            // Draw text without a window
            draw_list.add_text([100.0, 240.0], [1.0, 1.0, 1.0, 1.0], "Direct text rendering!");

            // Optional: Show debug window
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
