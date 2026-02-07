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

    // Example 1: Using add_fonts with embedded font data
    // Uncomment if you have a font file:
    // let font_data = include_bytes!("../path/to/your/font.ttf");
    // overlay.add_fonts(&[FontSource::TtfData {
    //     data: font_data,
    //     size_pixels: 18.0,
    //     config: None,
    // }]);

    // Example 2: Using configure_fonts for advanced configuration
    overlay.configure_fonts(|imgui| {
        let fonts = imgui.fonts();

        // Add default font with larger size
        fonts.add_font(&[FontSource::DefaultFontData {
            config: Some(FontConfig {
                size_pixels: 20.0,
                oversample_h: 2,
                oversample_v: 2,
                ..FontConfig::default()
            }),
        }]);

        // You can add multiple fonts with different sizes
        fonts.add_font(&[FontSource::DefaultFontData {
            config: Some(FontConfig {
                size_pixels: 14.0,
                ..FontConfig::default()
            }),
        }]);
    });

    // Main render loop
    loop {
        if !overlay.start_render() {
            break;
        }

        // Render UI
        overlay.render(|ui| {
            Window::new("Custom Font Example")
                .size([400.0, 300.0], Condition::FirstUseEver)
                .build(ui, || {
                    ui.text("This text uses the custom font!");
                    ui.separator();

                    // Use the first font (index 0)
                    let font = ui.push_font(ui.fonts().fonts()[0]);
                    ui.text("Large font (20px)");
                    font.pop();

                    ui.separator();

                    // Use the second font if available
                    if ui.fonts().fonts().len() > 1 {
                        let font = ui.push_font(ui.fonts().fonts()[1]);
                        ui.text("Smaller font (14px)");
                        font.pop();
                    }

                    ui.separator();
                    ui.text("You can switch fonts at runtime!");

                    if ui.button("Reset to default font") {
                        println!("Font reset requested");
                    }
                });
        });
    }

    println!("Overlay shutting down");
}
