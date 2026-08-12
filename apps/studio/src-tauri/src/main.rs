// Windows release builds open no console window; without this a GUI app flashes a
// terminal behind itself on launch.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    md_studio_lib::run()
}
