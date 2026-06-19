//! Tool discovery for native verification commands.

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacOs,
}

#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub platform: Platform,
    pub exe_ext: &'static str,
    pub llvm_target: &'static str,
    pub llvm_bin: Option<PathBuf>,
    pub clang: Option<PathBuf>,
    pub llvm_as: Option<PathBuf>,
    pub llvm_dis: Option<PathBuf>,
    pub llvm_link: Option<PathBuf>,
    pub cxxfilt: Option<PathBuf>,
    pub saw: Option<PathBuf>,
    pub solver_dir: Option<PathBuf>,
    pub exception_lower: Option<PathBuf>,
}

impl ToolPaths {
    pub fn discover() -> Self {
        let overrides = load_user_env_overrides();
        let platform = current_platform();
        let exe_ext = if platform == Platform::Windows {
            ".exe"
        } else {
            ""
        };
        let llvm_target = default_llvm_target(platform);
        let path_clang = find_on_path(&format!("clang{exe_ext}"));
        let llvm_bin = llvm_bin_candidates(platform, exe_ext, &overrides, path_clang.as_deref())
            .into_iter()
            .find(|dir| {
                has_tool(dir, &format!("clang{exe_ext}"))
                    && has_tool(dir, &format!("llvm-as{exe_ext}"))
            });
        let clang = llvm_bin
            .as_ref()
            .and_then(|dir| join_tool(dir, "clang", exe_ext));
        let llvm_as = llvm_bin
            .as_ref()
            .and_then(|dir| join_tool(dir, "llvm-as", exe_ext));
        let llvm_dis = llvm_bin
            .as_ref()
            .and_then(|dir| join_tool(dir, "llvm-dis", exe_ext));
        let llvm_link = llvm_bin
            .as_ref()
            .and_then(|dir| join_tool(dir, "llvm-link", exe_ext));
        let saw = saw_candidates(exe_ext, &overrides)
            .into_iter()
            .find(|p| p.exists());
        let solver_dir = solver_dir_candidates(exe_ext, &overrides, saw.as_deref())
            .into_iter()
            .find(|dir| has_tool(dir, &format!("z3{exe_ext}")));
        let cxxfilt = find_demangler(platform, exe_ext);
        let exception_lower = exception_lower_candidates(exe_ext, &overrides)
            .into_iter()
            .find(|p| p.exists());
        Self {
            platform,
            exe_ext,
            llvm_target,
            llvm_bin,
            clang,
            llvm_as,
            llvm_dis,
            llvm_link,
            cxxfilt,
            saw,
            solver_dir,
            exception_lower,
        }
    }

    pub fn add_discovered_dirs_to_path(&self) {
        prepend_path(self.solver_dir.as_deref());
        prepend_path(self.llvm_bin.as_deref());
    }
}

fn current_platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Linux
    }
}

fn default_llvm_target(platform: Platform) -> &'static str {
    match platform {
        Platform::Windows => "x86_64-pc-windows-msvc",
        Platform::Linux => "x86_64-unknown-linux-gnu",
        Platform::MacOs => {
            if cfg!(target_arch = "aarch64") {
                "aarch64-apple-darwin"
            } else {
                "x86_64-apple-darwin"
            }
        }
    }
}

fn load_user_env_overrides() -> HashMap<String, String> {
    let mut out = HashMap::new();
    for key in [
        "SAW_SPEC_GEN_LLVM_BIN",
        "SAW_SPEC_GEN_SAW",
        "SAW_SPEC_GEN_SOLVER_BIN",
        "SAW_SPEC_GEN_EXCEPTION_LOWER",
    ] {
        if let Ok(v) = env::var(key) {
            out.insert(key.to_string(), v);
        }
    }
    let Some(path) = user_env_path() else {
        return out;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    for (key, value) in parse_env_text(&text) {
        out.entry(key).or_insert(value);
    }
    out
}

fn user_env_path() -> Option<PathBuf> {
    let home = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".saw-spec-gen").join("env.ps1"))
}

fn parse_env_text(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("$env:") || !line.contains('=') {
            continue;
        }
        let Some((lhs, rhs)) = line.split_once('=') else {
            continue;
        };
        let key = lhs.trim().trim_start_matches("$env:").trim();
        let mut value = rhs.trim().trim_matches('"').trim_matches('\'').to_string();
        if value.ends_with('\r') {
            value.pop();
        }
        if !key.is_empty() && !value.is_empty() {
            out.push((key.to_string(), value));
        }
    }
    out
}

fn llvm_bin_candidates(
    platform: Platform,
    exe_ext: &str,
    overrides: &HashMap<String, String>,
    path_clang: Option<&Path>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(v) = overrides.get("SAW_SPEC_GEN_LLVM_BIN") {
        out.push(PathBuf::from(v));
    }
    if let Some(clang) = path_clang.and_then(Path::parent) {
        out.push(clang.to_path_buf());
    }
    match platform {
        Platform::Windows => {
            out.push(PathBuf::from(r"C:\Program Files\LLVM\bin"));
            if let Some(local) = env::var_os("LOCALAPPDATA") {
                out.push(
                    PathBuf::from(local)
                        .join("Programs")
                        .join("LLVM")
                        .join("bin"),
                );
            }
            if let Some(home) = env::var_os("USERPROFILE") {
                out.push(
                    PathBuf::from(home)
                        .join(".saw-spec-gen")
                        .join("llvm")
                        .join("bin"),
                );
            }
        }
        Platform::MacOs => {
            out.push(PathBuf::from("/opt/homebrew/opt/llvm/bin"));
            out.push(PathBuf::from("/usr/local/opt/llvm/bin"));
            if let Some(home) = env::var_os("HOME") {
                out.push(
                    PathBuf::from(home)
                        .join(".saw-spec-gen")
                        .join("llvm")
                        .join("bin"),
                );
            }
        }
        Platform::Linux => {
            out.push(PathBuf::from("/usr/lib/llvm-20/bin"));
            out.push(PathBuf::from("/usr/lib/llvm-19/bin"));
            out.push(PathBuf::from("/usr/lib/llvm-18/bin"));
            out.push(PathBuf::from("/usr/local/lib/llvm/bin"));
            if let Some(home) = env::var_os("HOME") {
                out.push(
                    PathBuf::from(home)
                        .join(".saw-spec-gen")
                        .join("llvm")
                        .join("bin"),
                );
            }
        }
    }
    out.retain(|p| p.exists() || has_tool(p, &format!("clang{exe_ext}")));
    out
}

fn saw_candidates(exe_ext: &str, overrides: &HashMap<String, String>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(v) = overrides.get("SAW_SPEC_GEN_SAW") {
        out.push(PathBuf::from(v));
    }
    if let Some(p) = find_on_path(&format!("saw{exe_ext}")) {
        out.push(p);
    }
    if let Some(home) = env::var_os("USERPROFILE") {
        out.push(
            PathBuf::from(home)
                .join(".saw-spec-gen")
                .join("saw")
                .join("bin")
                .join(format!("saw{exe_ext}")),
        );
    }
    if let Some(home) = env::var_os("HOME") {
        out.push(
            PathBuf::from(home)
                .join(".saw-spec-gen")
                .join("saw")
                .join("bin")
                .join(format!("saw{exe_ext}")),
        );
    }
    out
}

fn solver_dir_candidates(
    exe_ext: &str,
    overrides: &HashMap<String, String>,
    saw: Option<&Path>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(v) = overrides.get("SAW_SPEC_GEN_SOLVER_BIN") {
        out.push(PathBuf::from(v));
    }
    if let Some(dir) = saw.and_then(Path::parent) {
        out.push(dir.to_path_buf());
    }
    if let Some(z3) =
        find_on_path(&format!("z3{exe_ext}")).and_then(|p| p.parent().map(Path::to_path_buf))
    {
        out.push(z3);
    }
    if let Some(home) = env::var_os("USERPROFILE") {
        out.push(
            PathBuf::from(home)
                .join(".saw-spec-gen")
                .join("saw")
                .join("bin"),
        );
    }
    if let Some(home) = env::var_os("HOME") {
        out.push(
            PathBuf::from(home)
                .join(".saw-spec-gen")
                .join("saw")
                .join("bin"),
        );
    }
    out
}

fn exception_lower_candidates(exe_ext: &str, overrides: &HashMap<String, String>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(v) = overrides.get("SAW_SPEC_GEN_EXCEPTION_LOWER") {
        out.push(PathBuf::from(v));
    }
    if let Some(p) = find_on_path(&format!("exception-lower{exe_ext}")) {
        out.push(p);
    }
    for home_key in ["USERPROFILE", "HOME"] {
        if let Some(home) = env::var_os(home_key) {
            let home = PathBuf::from(home)
                .join(".saw-spec-gen")
                .join("exception-lower");
            out.push(home.join("bin").join(format!("exception-lower{exe_ext}")));
            out.push(home.join("build").join(format!("exception-lower{exe_ext}")));
        }
    }
    out
}

fn find_demangler(platform: Platform, exe_ext: &str) -> Option<PathBuf> {
    if platform == Platform::Windows {
        return find_on_path(&format!("undname{exe_ext}"));
    }
    find_on_path("c++filt").or_else(|| find_on_path("llvm-cxxfilt"))
}

fn join_tool(dir: &Path, name: &str, exe_ext: &str) -> Option<PathBuf> {
    let path = dir.join(format!("{name}{exe_ext}"));
    path.exists().then_some(path)
}

fn has_tool(dir: &Path, name: &str) -> bool {
    dir.join(name).exists()
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.exists())
}

fn prepend_path(dir: Option<&Path>) {
    let Some(dir) = dir else { return };
    let dir_text = dir.to_string_lossy();
    let sep = if cfg!(windows) { ";" } else { ":" };
    let path = env::var("PATH").unwrap_or_default();
    if !path.contains(dir_text.as_ref()) {
        env::set_var("PATH", format!("{dir_text}{sep}{path}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_env_lines() {
        let overrides = parse_env_text(
            "$env:SAW_SPEC_GEN_SAW = '/tmp/saw'\n$env:SAW_SPEC_GEN_LLVM_BIN = '/tmp/llvm'\n",
        );
        let map = overrides.into_iter().collect::<HashMap<_, _>>();
        assert_eq!(
            map.get("SAW_SPEC_GEN_SAW").map(String::as_str),
            Some("/tmp/saw")
        );
        assert_eq!(
            map.get("SAW_SPEC_GEN_LLVM_BIN").map(String::as_str),
            Some("/tmp/llvm")
        );
    }
}
