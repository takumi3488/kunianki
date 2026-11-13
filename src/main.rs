// Prevent console window in Windows release builds when launching from a file manager.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    kunianki::run(None)
}
