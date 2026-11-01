fn main() {
    let mut w = minifb::Window::new("hello", 640, 480, Default::default()).unwrap();
    let _ = w.update_with_buffer(&vec![0u32; 640 * 480], 640, 480);
}
