use std::process::Command;

/// The machine and OS, in words, for a report a person pastes into a PR. Best effort: a tool that
/// is not there leaves a question mark, and `--machine` overrides the model.
pub fn describe(model: Option<&str>) -> String {
    let model = model.map(str::to_owned).or_else(model_name).unwrap_or_else(|| "?".into());
    let os = os_name().unwrap_or_else(|| std::env::consts::OS.into());
    format!("{model}, {os}, {}", std::env::consts::ARCH)
}

/// The commit this runs from, with `-dirty` when the tree has changed since.
pub fn commit() -> String {
    output("git", &["describe", "--always", "--dirty", "--abbrev=7"]).unwrap_or_else(|| "?".into())
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !text.is_empty()).then_some(text)
}

#[cfg(target_os = "macos")]
fn model_name() -> Option<String> {
    output("sysctl", &["-n", "hw.model"])
}

#[cfg(target_os = "macos")]
fn os_name() -> Option<String> {
    output("sw_vers", &["-productVersion"]).map(|version| format!("macOS {version}"))
}

#[cfg(target_os = "linux")]
fn model_name() -> Option<String> {
    let name = std::fs::read_to_string("/sys/class/dmi/id/product_name").ok()?;
    Some(name.trim().to_owned())
}

#[cfg(target_os = "linux")]
fn os_name() -> Option<String> {
    let release = std::fs::read_to_string("/etc/os-release").ok()?;
    let line = release.lines().find_map(|line| line.strip_prefix("PRETTY_NAME="))?;
    Some(line.trim_matches('"').to_owned())
}

#[cfg(windows)]
fn model_name() -> Option<String> {
    let query = "(Get-CimInstance Win32_ComputerSystem).Model";
    output("powershell", &["-NoProfile", "-Command", query])
}

#[cfg(windows)]
fn os_name() -> Option<String> {
    output("cmd", &["/C", "ver"])
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn model_name() -> Option<String> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn os_name() -> Option<String> {
    None
}
