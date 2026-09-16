//! Shared artifact cache for the native C++ verification pipeline.

use crate::object_layout::capture;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct AstCacheContext {
    pub hit: bool,
    dir: PathBuf,
    bc_path: PathBuf,
    ll_path: PathBuf,
    layout_path: PathBuf,
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
        // v3 uses full SHA-256 keys and requires compiler layout facts. Preserve
        // all field/argument boundaries, including caller-supplied fingerprints.
        let key = format!(
            "v4|{}",
            serde_json::to_string(&(cpp_file, user_clang_flags, llvm_target, newest))?
        );
        let hash = short_hash(&key);
        let dir = output_dir
            .parent()
            .unwrap_or(output_dir)
            .join(".astcache")
            .join(hash);
        let bc_path = dir.join(format!("{base_name}.bc"));
        let ll_path = dir.join(format!("{base_name}.ll"));
        let layout_path = capture::sidecar_path(&ll_path);
        let ast_path = dir.join(format!("{base_name}_ast.json"));
        let mut ctx = Self {
            hit: false,
            dir,
            bc_path,
            ll_path,
            layout_path,
            ast_path,
            ll_file: Some(ll_file.to_path_buf()),
        };
        if ctx.bc_path.is_file()
            && ctx.ll_path.is_file()
            && ctx.layout_path.is_file()
            && ctx.ast_path.is_file()
        {
            validated_sidecar(&ctx.ll_path)?;
            std::fs::copy(&ctx.bc_path, bc_file)?;
            std::fs::copy(&ctx.ll_path, ll_file)?;
            std::fs::copy(&ctx.layout_path, capture::sidecar_path(ll_file))?;
            std::fs::copy(&ctx.ast_path, ast_file)?;
            ctx.hit = true;
        }
        Ok(ctx)
    }

    pub fn save(&self, bc_file: &Path, ll_file: Option<&Path>, ast_file: &Path) -> Result<()> {
        let ll_file = ll_file.context("cannot cache C++ artifacts without LLVM IR")?;
        let layout_file = validated_sidecar(ll_file)?;
        std::fs::create_dir_all(&self.dir)?;
        // Publish the required sidecar last so an interrupted overwrite cannot
        // leave an otherwise complete entry paired with an old build's facts.
        if self.layout_path.try_exists()? {
            std::fs::remove_file(&self.layout_path)?;
        }
        std::fs::copy(bc_file, &self.bc_path)?;
        std::fs::copy(ll_file, &self.ll_path)?;
        std::fs::copy(ast_file, &self.ast_path)?;
        std::fs::copy(layout_file, &self.layout_path)?;
        Ok(())
    }
}

fn validated_sidecar(ll_file: &Path) -> Result<PathBuf> {
    let ir = std::fs::read_to_string(ll_file)
        .with_context(|| format!("reading LLVM IR {} for cache layouts", ll_file.display()))?;
    capture::load(ll_file, &ir)?
        .with_context(|| format!("missing compiler layouts for {}", ll_file.display()))?;
    Ok(capture::sidecar_path(ll_file))
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
    format!("{:x}", Sha256::digest(input.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, thread, time::Duration};

    const TARGET: &str = "x86_64-pc-windows-msvc";
    const IR: &str = r#"target datalayout = "e-m:w-p:64:64-i64:64-n8:16:32:64-S128"
target triple = "x86_64-pc-windows-msvc"
%struct.Foo = type { i32 }
!llvm.ident = !{!0}
!0 = !{!"clang version cache-test"}
"#;
    const DUMP: &str = "*** Dumping AST Record Layout\n         0 | struct Foo\n         0 |   int value\n           | [sizeof=4, align=4,\n           |  nvsize=4, nvalign=4]\n";

    struct CacheFixture {
        _dir: tempdir_compat::TempDir,
        cpp: PathBuf,
        output: PathBuf,
        bc: PathBuf,
        ll: PathBuf,
        ast: PathBuf,
    }

    impl CacheFixture {
        fn new() -> Self {
            let dir = tempdir_compat::TempDir::new("cache-layout").unwrap();
            let cpp = dir.path().join("f.cpp");
            let output = dir.path().join("out");
            let bc = output.join("f.bc");
            let ll = output.join("f.ll");
            let ast = output.join("f_ast.json");
            fs::create_dir_all(&output).unwrap();
            fs::write(&cpp, "struct Foo { int value; };").unwrap();
            fs::write(&bc, "bitcode").unwrap();
            fs::write(&ll, IR).unwrap();
            fs::write(&ast, "{}").unwrap();
            let facts = capture::capture(IR, DUMP, vec!["/toolchain/clang".into()]).unwrap();
            capture::write(&ll, &facts).unwrap();
            Self {
                _dir: dir,
                cpp,
                output,
                bc,
                ll,
                ast,
            }
        }

        fn load(&self, flags: &[String]) -> Result<AstCacheContext> {
            AstCacheContext::load(
                &self.cpp,
                &[],
                flags,
                TARGET,
                &self.output,
                "f",
                &self.bc,
                &self.ll,
                &self.ast,
            )
        }
    }

    #[test]
    fn round_trips_ir_and_layout_sidecar_with_cached_artifacts() {
        let f = CacheFixture::new();
        let cache = f.load(&[]).unwrap();
        assert!(!cache.hit);
        let layout = capture::sidecar_path(&f.ll);
        let expected = fs::read(&layout).unwrap();
        cache.save(&f.bc, Some(&f.ll), &f.ast).unwrap();
        assert_eq!(fs::read(&cache.layout_path).unwrap(), expected);
        for path in [&f.bc, &f.ll, &f.ast, &layout] {
            fs::remove_file(path).unwrap();
        }
        let hit = f.load(&[]).unwrap();
        assert!(hit.hit);
        assert_eq!(hit.ll_file.as_deref(), Some(f.ll.as_path()));
        assert_eq!(fs::read_to_string(&f.bc).unwrap(), "bitcode");
        assert_eq!(fs::read_to_string(&f.ll).unwrap(), IR);
        assert_eq!(fs::read_to_string(&f.ast).unwrap(), "{}");
        assert_eq!(fs::read(layout).unwrap(), expected);
        assert_eq!(
            capture::load(&f.ll, IR).unwrap().unwrap().records["Foo"].size,
            4
        );
    }

    #[test]
    fn cache_preserves_original_facts_after_ir_transforms() {
        let f = CacheFixture::new();
        let layout = capture::sidecar_path(&f.ll);
        let original = fs::read(&layout).unwrap();
        let transformed = IR
            .lines()
            .filter(|line| !line.starts_with('!'))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n%eh.extra = type { ptr, i32 }\n";
        fs::write(&f.ll, &transformed).unwrap();
        let cache = f.load(&[]).unwrap();
        cache.save(&f.bc, Some(&f.ll), &f.ast).unwrap();
        fs::remove_file(&layout).unwrap();
        assert!(f.load(&[]).unwrap().hit);
        assert_eq!(fs::read(&layout).unwrap(), original);
        let facts = capture::load(&f.ll, &transformed).unwrap().unwrap();
        assert_eq!(facts.compiler, "clang version cache-test");
        assert!(!facts.llvm_types.contains_key("eh.extra"));
    }

    #[test]
    fn incomplete_cache_entries_are_misses_without_copying() {
        for missing in ["bc", "ll", "layout", "ast"] {
            let f = CacheFixture::new();
            let cache = f.load(&[]).unwrap();
            cache.save(&f.bc, Some(&f.ll), &f.ast).unwrap();
            let path = match missing {
                "bc" => &cache.bc_path,
                "ll" => &cache.ll_path,
                "layout" => &cache.layout_path,
                _ => &cache.ast_path,
            };
            fs::remove_file(path).unwrap();
            fs::write(&f.bc, "untouched output").unwrap();
            let miss = f.load(&[]).unwrap();
            assert!(!miss.hit, "missing {missing}");
            assert_eq!(miss.ll_file.as_deref(), Some(f.ll.as_path()));
            assert_eq!(fs::read_to_string(&f.bc).unwrap(), "untouched output");
        }
    }

    #[test]
    fn short_hash_keeps_the_full_sha256_digest() {
        assert_eq!(
            short_hash(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            short_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn v3_ignores_legacy_cache_keys_even_with_complete_artifacts() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let f = CacheFixture::new();
        let newest = newest_timestamp(&f.cpp, &[]).unwrap();
        for key in [
            format!("{}||{TARGET}|{newest}", f.cpp.display()),
            format!("v2|{}|[]|{TARGET}|{newest}", f.cpp.display()),
        ] {
            let mut old_hasher = DefaultHasher::new();
            key.hash(&mut old_hasher);
            // Reject actual old keys, and old schemas even if hashed with SHA-256.
            for hash in [format!("{:016x}", old_hasher.finish()), short_hash(&key)] {
                let legacy = f.output.parent().unwrap().join(".astcache").join(hash);
                fs::create_dir_all(&legacy).unwrap();
                for path in [&f.bc, &f.ll, &f.ast, &capture::sidecar_path(&f.ll)] {
                    fs::copy(path, legacy.join(path.file_name().unwrap())).unwrap();
                }
                let cache = f.load(&[]).unwrap();
                assert_ne!(cache.dir, legacy);
                assert!(!cache.hit);
            }
        }
    }

    #[test]
    fn caller_fingerprints_and_argument_boundaries_change_cache_keys() {
        let f = CacheFixture::new();
        let first = vec!["compiler fingerprint A".into()];
        let cache = f.load(&first).unwrap();
        cache.save(&f.bc, Some(&f.ll), &f.ast).unwrap();
        assert!(f.load(&first).unwrap().hit);
        assert!(!f.load(&["compiler fingerprint B".into()]).unwrap().hit);
        assert!(
            !f.load(&["compiler".into(), "fingerprint A".into()])
                .unwrap()
                .hit
        );
    }

    #[test]
    fn cannot_save_without_ir_and_valid_layouts() {
        let f = CacheFixture::new();
        let cache = f.load(&[]).unwrap();
        assert!(cache.save(&f.bc, None, &f.ast).is_err());
        let layout = capture::sidecar_path(&f.ll);
        fs::remove_file(&layout).unwrap();
        assert!(cache.save(&f.bc, Some(&f.ll), &f.ast).is_err());
        fs::write(&layout, "invalid JSON").unwrap();
        assert!(cache.save(&f.bc, Some(&f.ll), &f.ast).is_err());
        assert!(!cache.dir.exists());
    }

    #[test]
    fn malformed_and_stale_cached_layouts_fail_closed() {
        for stale_ir in [
            None,
            Some(IR.replace("{ i32 }", "{ i64 }")),
            Some(IR.replace(TARGET, "aarch64-unknown-linux-gnu")),
            Some(IR.replace("e-m:w-p:64:64-i64:64-n8:16:32:64-S128", "E-p:32:32")),
            Some(IR.replace("%struct.Foo = type { i32 }\n", "")),
        ] {
            let f = CacheFixture::new();
            let cache = f.load(&[]).unwrap();
            cache.save(&f.bc, Some(&f.ll), &f.ast).unwrap();
            if let Some(stale_ir) = stale_ir {
                fs::write(&cache.ll_path, stale_ir).unwrap();
            } else {
                fs::write(&cache.layout_path, "invalid JSON").unwrap();
            }
            fs::write(&f.bc, "untouched output").unwrap();
            assert!(f.load(&[]).is_err());
            assert_eq!(fs::read_to_string(&f.bc).unwrap(), "untouched output");
        }
    }

    #[test]
    fn failed_cache_overwrite_does_not_leave_old_layouts_reusable() {
        let f = CacheFixture::new();
        let cache = f.load(&[]).unwrap();
        cache.save(&f.bc, Some(&f.ll), &f.ast).unwrap();
        fs::write(&f.bc, "new bitcode").unwrap();
        fs::remove_file(&f.ast).unwrap();
        assert!(cache.save(&f.bc, Some(&f.ll), &f.ast).is_err());
        assert!(!cache.layout_path.exists());
        assert!(!f.load(&[]).unwrap().hit);
    }

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
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::{env, fs, io, path::PathBuf};

        pub struct TempDir(pub PathBuf);

        impl TempDir {
            pub fn new(prefix: &str) -> io::Result<Self> {
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let mut p = env::temp_dir();
                let n = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                p.push(format!(
                    "saw-spec-gen-{prefix}-{}-{n}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
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
