use truce_rack_core::info::PluginInfo;

/// True when a loaded plugin corresponds to connected hardware.
pub fn plugin_name_matches_device(plugin: &str, device: &str) -> bool {
    let p = plugin.to_ascii_lowercase();
    let d = device.to_ascii_lowercase();
    if p.contains(&d) || d.contains(&p) {
        return true;
    }
    for key in [
        "analog heat",
        "digitakt",
        "syntakt",
        "digitone",
        "analog rytm",
        "analog four",
        "analog keys",
    ] {
        if p.contains(key) && d.contains(key) {
            return true;
        }
    }
    false
}

/// Pick the best plugin entry for a connected device name.
pub fn best_plugin_for_device<'a>(
    device_name: &str,
    plugins: &'a [PluginInfo],
) -> Option<&'a PluginInfo> {
    let d = device_name.to_ascii_lowercase();
    let want_fx = d.contains("+fx") || d.contains(" fx");

    plugins
        .iter()
        .filter(|p| plugin_name_matches_device(&p.name, device_name))
        .max_by_key(|p| {
            let name = p.name.to_ascii_lowercase();
            let mut score = 0i32;
            if plugin_name_matches_device(&p.name, device_name) {
                score += 100;
            }
            if want_fx && name.contains("+fx") {
                score += 50;
            } else if !want_fx && !name.contains("+fx") {
                score += 40;
            }
            if name.contains("mkii") && d.contains("mkii") {
                score += 10;
            }
            score -= name.len() as i32;
            score
        })
}
