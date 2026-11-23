use std::process::Command;

/// Name of the focused output, if Hyprland is running.
pub fn focused_output() -> Option<String> {
    let out = Command::new("hyprctl")
        .args(["-j", "activeworkspace"])
        .output()
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    v["monitor"].as_str().map(str::to_owned)
}
