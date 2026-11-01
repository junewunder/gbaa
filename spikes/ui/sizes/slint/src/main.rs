slint::slint! {
    import { Button } from "std-widgets.slint";
    export component App inherits Window {
        Button { text: "hello"; }
    }
}

fn main() {
    App::new().unwrap().run().unwrap();
}
