use leptos::prelude::mount_to_body;
use oxid_web::app::App;

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}
