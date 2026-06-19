use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacOs,
}

#[derive(Debug, Clone)]
pub struct DiscoveredTools {
    pub platform: Platform,
    pub exe_ext: &'static str,
    pub llvm_target: String,
    pub llvm_bin: Option<PathBuf>,
    pub llvm_dis: Option<PathBuf>,
    pub saw: Option<PathBuf>,
    pub solver_dir: Option<PathBuf>,
    pub rustc: Option<PathBuf>,
    pub exception_lower: Option<PathBuf>,
}

impl DiscoveredTools {
    pub fn require_verify_rust(&self) -> Result<()> {
        let mut missing = Vec::new();
        if self.llvm_dis.is_none() {
            missing.push("LlvmDis");
        }
        if self.saw.is_none() {
            missing.push("Saw");
        }
        if self.rustc.is_none() {
            missing.push("Rustc");
        }
        if missing.is_empty() {
            return Ok(());
        }
        bail!(
            "saw-spec-gen could not find required tool(s): {}",
            missing.join(", ")
        )
    }

    pub fn add_solver_and_llvm_to_path(&self) {
        let mut parts = vec![];
        if let Some(dir) = &self.solver_dir {
            if dir.exists() {
                parts.push(dir.clone());
            }
        }
        if let Some(dir) = &self.llvm_bin {
            if dir.exists() {
                parts.push(dir.clone());
            }
        }
        if parts.is_empty() {
            return;
        }
        let old = env::var_os("PATH").unwrap_or_default();
        let mut new_path = env::split_paths(&old).collect::<Vec<_>>();
        for p in parts.into_iter().rev() {
            if !new_path.iter().any(|x| x == &p) {
                new_path.insert(0, p);
            }
        }
        if let Ok(joined) = env::join_paths(new_path) {
            // SAFETY: process-local environment mutation is intentional
            // here and done by this single-threaded CLI before any
            // concurrent worker threads are spawned.
            unsafe { env::set_var("PATH", joined) };
        }
    }
}

pub fn discover_tools() -> Result<DiscoveredTools> {
    let platform = detect_platform();
    let exe_ext = if platform == Platform::Windows {
        ".exe"
    } else {
        ""
    };
    let env_file = read_user_env_file_vars()?;

    let llvm_target = read_with_file_fallback("SAW_SPEC_GEN_LLVM_TARGET", &env_file)
        .unwrap_or_else(|| default_llvm_target(platform));

    let llvm_bin = discover_llvm_bin(platform, exe_ext, &env_file);
    let llvm_dis = resolve_tool_path("llvm-dis", exe_ext, llvm_bin.as_deref(), &env_file, None);

    let saw = resolve_tool_path(
        "saw",
        exe_ext,
        None,
        &env_file,
        Some(default_saw_candidates(exe_ext)),
    );
    let rustc = resolve_tool_path("rustc", exe_ext, None, &env_file, None);

    let solver_dir = discover_solver_dir(exe_ext, &env_file, saw.as_ref().and_then(|p| p.parent()));
    let exception_lower = resolve_tool_path(
        "exception-lower",
        exe_ext,
        None,
        &env_file,
        Some(default_exception_lower_candidates(exe_ext)),
    );

    Ok(DiscoveredTools {
        platform,
        exe_ext,
        llvm_target,
        llvm_bin,
        llvm_dis,
        saw,
        solver_dir,
        rustc,
        exception_lower,
    })
}

fn detect_platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Linux
    }
}

fn default_llvm_target(platform: Platform) -> String {
    match platform {
        Platform::Windows => "x86_64-pc-windows-msvc".to_string(),
        Platform::Linux => "x86_64-unknown-linux-gnu".to_string(),
        Platform::MacOs => {
            if cfg!(target_arch = "aarch64") {
                "aarch64-apple-darwin".to_string()
            } else {
                "x86_64-apple-darwin".to_string()
            }
        }
    }
}

fn read_user_env_file_vars() -> Result<HashMap<String, String>> {
    let Some(home) = user_home_dir() else {
        return Ok(HashMap::new());
    };
    let env_file = home.join(".saw-spec-gen").join("env.ps1");
    if !env_file.exists() {
        return Ok(HashMap::new());
    }
    let text = fs::read_to_string(&env_file)
        .with_context(|| format!("failed to read {}", env_file.display()))?;
    let mut vars = HashMap::new();
    for line in text.lines() {
        let s = line.trim();
        if !s.starts_with("$env:SAW_SPEC_GEN_") {
            continue;
        }
        let Some((lhs, rhs)) = s.split_once('=') else {
            continue;
        };
        let key = lhs.trim().trim_start_matches("$env:").to_string();
        if env::var_os(&key).is_some() {
            continue;
        }
        let mut value = rhs.trim().to_string();
        if value.len() >= 2
            && ((value.starts_with('\'') && value.ends_with('\''))
                || (value.starts_with('"') && value.ends_with('"')))
        {
            value = value[1..value.len() - 1].to_string();
        }
        if !value.is_empty() {
            vars.insert(key, value);
        }
    }
    Ok(vars)
}

fn read_with_file_fallback(key: &str, file_vars: &HashMap<String, String>) -> Option<String> {
    env::var(key).ok().or_else(|| file_vars.get(key).cloned())
}

fn resolve_tool_path(
    base: &str,
    exe_ext: &str,
    dir_hint: Option<&Path>,
    file_vars: &HashMap<String, String>,
    default_candidates: Option<Vec<PathBuf>>,
) -> Option<PathBuf> {
    let env_key = format!(
        "SAW_SPEC_GEN_{}",
        base.replace('-', "_").to_ascii_uppercase()
    );
    if let Some(v) = read_with_file_fallback(&env_key, file_vars) {
        let p = PathBuf::from(v);
        if p.exists() {
            if base == "rustc" {
                return Some(p);
            }
            return fs::canonicalize(p).ok();
        }
    }

    if let Some(dir) = dir_hint {
        let p = dir.join(format!("{base}{exe_ext}"));
        if p.exists() {
            if base == "rustc" {
                return Some(p);
            }
            return fs::canonicalize(p).ok();
        }
    }

    if let Some(p) = find_on_path(base, exe_ext) {
        return Some(p);
    }

    if let Some(cands) = default_candidates {
        for p in cands {
            if p.exists() {
                if base == "rustc" {
                    return Some(p);
                }
                return fs::canonicalize(p).ok();
            }
        }
    }
    None
}

fn discover_llvm_bin(
    platform: Platform,
    exe_ext: &str,
    file_vars: &HashMap<String, String>,
) -> Option<PathBuf> {
    let mut candidates = Vec::<PathBuf>::new();
    if let Some(v) = read_with_file_fallback("SAW_SPEC_GEN_LLVM_BIN", file_vars) {
        candidates.push(PathBuf::from(v));
    }
    if let Some(path_clang) = find_on_path("clang", exe_ext) {
        if let Some(parent) = path_clang.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    match platform {
        Platform::Windows => {
            candidates.push(PathBuf::from(r"C:\Program Files\LLVM\bin"));
            if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
                candidates.push(PathBuf::from(local_app_data).join("Programs/LLVM/bin"));
            }
            if let Some(home) = user_home_dir() {
                candidates.push(home.join(".saw-spec-gen/llvm/bin"));
            }
        }
        Platform::MacOs => {
            candidates.push(PathBuf::from("/opt/homebrew/opt/llvm/bin"));
            candidates.push(PathBuf::from("/usr/local/opt/llvm/bin"));
            if let Some(home) = user_home_dir() {
                candidates.push(home.join(".saw-spec-gen/llvm/bin"));
            }
        }
        Platform::Linux => {
            candidates.push(PathBuf::from("/usr/lib/llvm-20/bin"));
            candidates.push(PathBuf::from("/usr/lib/llvm-19/bin"));
            candidates.push(PathBuf::from("/usr/lib/llvm-18/bin"));
            candidates.push(PathBuf::from("/usr/local/lib/llvm/bin"));
            if let Some(home) = user_home_dir() {
                candidates.push(home.join(".saw-spec-gen/llvm/bin"));
            }
        }
    }

    for dir in candidates {
        let clang = dir.join(format!("clang{exe_ext}"));
        let llvm_as = dir.join(format!("llvm-as{exe_ext}"));
        if clang.exists() && llvm_as.exists() {
            return fs::canonicalize(dir).ok();
        }
    }
    None
}

fn discover_solver_dir(
    exe_ext: &str,
    file_vars: &HashMap<String, String>,
    saw_parent: Option<&Path>,
) -> Option<PathBuf> {
    let mut candidates = Vec::<PathBuf>::new();
    if let Some(v) = read_with_file_fallback("SAW_SPEC_GEN_SOLVER_BIN", file_vars) {
        candidates.push(PathBuf::from(v));
    }
    if let Some(parent) = saw_parent {
        candidates.push(parent.to_path_buf());
    }
    if let Some(z3) = find_on_path("z3", exe_ext) {
        if let Some(parent) = z3.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    if let Some(home) = user_home_dir() {
        candidates.push(home.join(".saw-spec-gen/saw/bin"));
    }
    if let Some(user_profile) = env::var_os("USERPROFILE") {
        candidates.push(PathBuf::from(user_profile).join(".saw-spec-gen/saw/bin"));
    }
    for dir in candidates {
        if dir.join(format!("z3{exe_ext}")).exists() {
            return fs::canonicalize(dir).ok();
        }
    }
    None
}

fn default_saw_candidates(exe_ext: &str) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Some(home) = user_home_dir() {
        out.push(home.join(format!(".saw-spec-gen/saw/bin/saw{exe_ext}")));
    }
    if let Some(user_profile) = env::var_os("USERPROFILE") {
        out.push(
            PathBuf::from(user_profile).join(format!(".saw-spec-gen\\saw\\bin\\saw{exe_ext}")),
        );
    }
    out
}

fn default_exception_lower_candidates(exe_ext: &str) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Some(home) = user_home_dir() {
        out.push(home.join(format!(
            ".saw-spec-gen/exception-lower/bin/exception-lower{exe_ext}"
        )));
        out.push(home.join(format!(
            ".saw-spec-gen/exception-lower/build/exception-lower{exe_ext}"
        )));
    }
    if let Some(user_profile) = env::var_os("USERPROFILE") {
        let up = PathBuf::from(user_profile);
        out.push(up.join(format!(
            ".saw-spec-gen\\exception-lower\\bin\\exception-lower{exe_ext}"
        )));
        out.push(up.join(format!(
            ".saw-spec-gen\\exception-lower\\build\\exception-lower{exe_ext}"
        )));
    }
    out
}

fn find_on_path(base: &str, exe_ext: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let cand = dir.join(format!("{base}{exe_ext}"));
        if cand.exists() {
            return Some(cand);
        }
    }
    None
}

fn user_home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn default_target_is_non_empty() {
        assert!(!default_llvm_target(Platform::Windows).is_empty());
        assert!(!default_llvm_target(Platform::Linux).is_empty());
        assert!(!default_llvm_target(Platform::MacOs).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn rustc_env_path_keeps_symlink_name() {
        use std::os::unix::fs::symlink;

        let tmp = std::env::temp_dir().join(format!("saw-spec-gen-rustc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("create temp dir");
        let rustup = tmp.join("rustup");
        let rustc = tmp.join("rustc");
        fs::write(&rustup, "#!/bin/sh\n").expect("write rustup file");
        symlink(&rustup, &rustc).expect("create rustc symlink");

        let mut file_vars = HashMap::new();
        file_vars.insert(
            "SAW_SPEC_GEN_RUSTC".to_string(),
            rustc.to_string_lossy().to_string(),
        );
        let resolved =
            resolve_tool_path("rustc", "", None, &file_vars, None).expect("resolve rustc");
        assert_eq!(resolved, rustc);

        let _ = fs::remove_dir_all(&tmp);
    }
}
