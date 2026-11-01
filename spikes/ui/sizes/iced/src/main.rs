fn view(_: &u32) -> iced::Element<'_, ()> {
    iced::widget::button("hello").on_press(()).into()
}

fn main() -> iced::Result {
    iced::run(|_: &mut u32, _: ()| {}, view)
}
