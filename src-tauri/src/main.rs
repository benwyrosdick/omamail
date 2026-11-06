fn main() {
    match omamail_app::dispatch() {
        omamail_app::Launch::Gui => omamail_lib::run(),
        omamail_app::Launch::Exit(code) => std::process::exit(code),
    }
}
