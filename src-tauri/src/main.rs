// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--hook") {
        std::process::exit(regente_lib::hookcli::run(&args));
    }
    if args.iter().any(|a| a == "--statusline") {
        std::process::exit(regente_lib::statusline::run(&args));
    }
    regente_lib::run()
}
