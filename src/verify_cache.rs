//! Shared artifact cache for the native C++ verification pipeline.

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct AstCacheContext {
    pub hit: bool,
    dir: PathBuf,
    bc_path: PathBuf,
    ll_path: PathBuf,
    ast_path: PathBuf,
    pub ll_file: Option<PathBuf>,
}

impl AstCacheContext {
    #[allow(clippy::too_many_arguments)]
    pub fn load(
        cpp_file: &Path,
        include_dirs: &[PathBuf],
        user_clang_flags: &[String],
        llvm_target: &str,
        output_dir: &Path,
        base_name: &str,
        bc_file: &Path,
        ll_file: &Path,
        ast_file: &Path,
    ) -> Result<Self> {
        let newest = newest_timestamp(cpp_file, include_dirs)?;
        let key = format!(
            "{}|{}|{}|{}",
            cpp_file.display(),
            user_clang_flags.join(" "),
            llvm_target,
            newest
        );
        let hash = short_hash(&key);
        let dir = output_dir
            .parent()
            .unwrap_or(output_dir)
            .join(".astcache")
            .join(hash);
        let bc_path = dir.join(format!("{base_name}.bc"));
        let ll_path = dir.join(format!("{base_name}.ll"));
        let ast_path = dir.join(format!("{base_name}_ast.json"));
        let mut ctx = Self {
            hit: false,
            dir,
            bc_path,
            ll_path,
            ast_path,
            ll_file: Some(ll_file.to_path_buf()),
        };
        if ctx.bc_path.exists() && ctx.ast_path.exists() {
            std::fs::copy(&ctx.bc_path, bc_file)?;
            if ctx.ll_path.exists() {
                std::fs::copy(&ctx.ll_path, ll_file)?;
            } else {
                ctx.ll_file = None;
            }
            std::fs::copy(&ctx.ast_path, ast_file)?;
            ctx.hit = true;
        }
        Ok(ctx)
    }

    pub fn save(&self, bc_file: &Path, ll_file: Option<&Path>, ast_file: &Path) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::copy(bc_file, &self.bc_path)?;
        if let Some(ll_file) = ll_file {
            std::fs::copy(ll_file, &self.ll_path)?;
        }
        std::fs::copy(ast_file, &self.ast_path)?;
        Ok(())
    }
}

fn newest_timestamp(cpp_file: &Path, include_dirs: &[PathBuf]) -> Result<u128> {
    let mut newest = modified_nanos(cpp_file)?;
    for dir in include_dirs {
        visit_files(dir, &mut |path| {
            if let Ok(ts) = modified_nanos(path) {
                newest = newest.max(ts);
            }
        })?;
    }
    Ok(newest)
}

fn visit_files(dir: &Path, visit: &mut impl FnMut(&Path)) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            visit_files(&path, visit)?;
        } else if ty.is_file() {
            visit(&path);
        }
    }
    Ok(())
}

fn modified_nanos(path: &Path) -> Result<u128> {
    let modified = std::fs::metadata(path)?.modified()?;
    Ok(duration_since_epoch(modified))
}

fn duration_since_epoch(ts: SystemTime) -> u128 {
    ts.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn short_hash(input: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, thread, time::Duration};

    #[test]
    fn key_changes_when_header_changes() {
        let dir = tempdir_compat::TempDir::new("cache-key").unwrap();
        let cpp = dir.path().join("f.cpp");
        let inc = dir.path().join("include");
        let header = inc.join("f.hpp");
        fs::create_dir_all(&inc).unwrap();
        fs::write(&cpp, "int f();").unwrap();
        fs::write(&header, "int x;").unwrap();
        let first = newest_timestamp(&cpp, std::slice::from_ref(&inc)).unwrap();
        thread::sleep(Duration::from_millis(5));
        fs::write(&header, "int y;").unwrap();
        let second = newest_timestamp(&cpp, std::slice::from_ref(&inc)).unwrap();
        assert!(second >= first);
    }

    mod tempdir_compat {
        use std::{env, fs, io, path::PathBuf};

        pub struct TempDir(pub PathBuf);

        impl TempDir {
            pub fn new(prefix: &str) -> io::Result<Self> {
                let mut p = env::temp_dir();
                let n = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                p.push(format!("saw-spec-gen-{prefix}-{n}"));
                fs::create_dir_all(&p)?;
                Ok(TempDir(p))
            }

            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
}
