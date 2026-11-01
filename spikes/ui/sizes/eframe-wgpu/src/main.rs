fn main() -> eframe::Result {
    eframe::run_ui_native("hello", Default::default(), |ui, _| {
        let _ = ui.button("hello");
    })
}
