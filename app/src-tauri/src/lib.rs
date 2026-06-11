// The companion app's Tauri backend is intentionally empty: all real state
// lives in the REAPER extension, which the frontend talks to directly over
// the localhost control WebSocket.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
