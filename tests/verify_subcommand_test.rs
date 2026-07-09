#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn saw_spec_gen_binary() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("saw-spec-gen");
    path
}

#[test]
fn verify_subcommand_uses_cache_and_passes_clang_flags() {
    let env = FakeVerifyEnv::new(SawMode::Verified);
    let out_dir = env.root.path().join("out_verify");

    let status = env.run_verify(&out_dir);
    assert!(status.success(), "verify failed: {status:?}");

    let log = std::fs::read_to_string(&env.clang_log).unwrap();
    assert!(log.contains(&format!("-I {}", env.include_dir.display())));
    assert!(log.contains("-std=c++20"));
    assert!(log.contains("-DMOCK=1"));
    let first_count = log.lines().count();
    assert_eq!(first_count, 3, "expected bc + ll + ast clang invocations");

    let status = env.run_verify(&out_dir);
    assert!(status.success(), "second verify failed: {status:?}");
    let second_log = std::fs::read_to_string(&env.clang_log).unwrap();
    assert_eq!(
        second_log.lines().count(),
        first_count,
        "cache miss reran clang"
    );

    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("result.json")).unwrap())
            .unwrap();
    assert_eq!(result["schema_version"], "1");
    assert_eq!(result["verdict"], "VERIFIED");
    assert!(env
        .root
        .path()
        .join("implementation_inventory.json")
        .exists());
}

#[test]
fn verify_subcommand_writes_disproved_result_details() {
    let env = FakeVerifyEnv::new(SawMode::Disproved);
    let out_dir = env.root.path().join("out_disproved");

    let status = env.run_verify(&out_dir);
    assert_eq!(status.code(), Some(1));

    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("result.json")).unwrap())
            .unwrap();
    assert_eq!(result["schema_version"], "1");
    assert_eq!(result["verdict"], "DISPROVED");
    assert_eq!(result["counterexample"][0]["name"], "x");
    assert_eq!(result["counterexample"][0]["value"], "42");
    assert_eq!(result["expected"], "7");
    assert_eq!(result["actual"], "0");
}

#[test]
fn verify_subcommand_discovers_spec_sibling_config() {
    let env = FakeVerifyEnv::new(SawMode::Verified);
    let out_dir = env.root.path().join("out_config_auto");
    std::fs::write(
        env.cry_file.with_extension("toml"),
        r#"[functions.ComputeChecksum_spec]
max_len_precond = ["length=32"]
"#,
    )
    .unwrap();

    let status = env.run_verify_with_args(&out_dir, &[]);
    assert!(status.success(), "verify failed: {status:?}");

    let saw = std::fs::read_to_string(out_dir.join("verify.saw")).unwrap();
    assert!(
        saw.contains("llvm_precond {{ `32 >= length }};"),
        "expected config-driven max-len precondition in verify.saw, got:\n{saw}"
    );
    assert!(
        out_dir.join("fixture.cry").exists(),
        "copied spec artifact missing"
    );
}

#[test]
fn verify_subcommand_forwards_explicit_config_path() {
    let env = FakeVerifyEnv::new(SawMode::Verified);
    let out_dir = env.root.path().join("out_config_explicit");
    let config_dir = env.root.path().join("configs");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("verify.toml");
    std::fs::write(
        &config_path,
        r#"[functions.ComputeChecksum_spec]
max_len_precond = ["length=64"]
"#,
    )
    .unwrap();

    let status = env.run_verify_with_args(&out_dir, &["--config", config_path.to_str().unwrap()]);
    assert!(status.success(), "verify failed: {status:?}");

    let saw = std::fs::read_to_string(out_dir.join("verify.saw")).unwrap();
    assert!(
        saw.contains("llvm_precond {{ `64 >= length }};"),
        "expected explicit config path to reach gen-verify, got:\n{saw}"
    );
}

#[test]
fn verify_subcommand_forwards_shaping_flags_without_passthrough() {
    let env = FakeVerifyEnv::new(SawMode::Verified);
    let out_dir = env.root.path().join("out_shaping_flags");

    let status = env.run_verify_with_args(
        &out_dir,
        &[
            "--in-buffer-size",
            "data=32",
            "--max-len-precond",
            "length=32",
            "--no-struct-shape-recognizer",
        ],
    );
    assert!(status.success(), "verify failed: {status:?}");

    let saw = std::fs::read_to_string(out_dir.join("verify.saw")).unwrap();
    assert!(
        saw.contains("llvm_precond {{ `32 >= length }};"),
        "expected direct shaping flags to reach gen-verify, got:\n{saw}"
    );
}

struct FakeVerifyEnv {
    root: TempDir,
    cpp_file: PathBuf,
    cry_file: PathBuf,
    include_dir: PathBuf,
    clang_log: PathBuf,
    fake_bin: PathBuf,
}

enum SawMode {
    Verified,
    Disproved,
}

impl FakeVerifyEnv {
    fn new(mode: SawMode) -> Self {
        let root = TempDir::new("verify-subcommand").unwrap();
        let fake_bin = root.path().join("fakebin");
        let include_dir = root.path().join("include");
        std::fs::create_dir_all(&fake_bin).unwrap();
        std::fs::create_dir_all(&include_dir).unwrap();
        std::fs::write(include_dir.join("fixture.hpp"), "int fixture;\n").unwrap();

        let cpp_file = root.path().join("fixture.cpp");
        std::fs::write(
            &cpp_file,
            "unsigned ComputeChecksum(const unsigned char* data, unsigned long len) { return 0; }\n",
        )
        .unwrap();
        let cry_file = root.path().join("fixture.cry");
        std::fs::write(
            &cry_file,
            "module Main where\nComputeChecksum_spec : [32] -> [32]\nComputeChecksum_spec x = x\n",
        )
        .unwrap();
        let clang_log = root.path().join("clang.log");
        let ast_path = repo_root().join("tests/fixtures/request_validator_cpp.json");

        write_script(
            &fake_bin.join("clang"),
            &format!(
                r#"#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "{log}"
out=""
prev=""
for arg in "$@"; do
  if [[ "$prev" == "-o" ]]; then out="$arg"; fi
  prev="$arg"
done
if [[ " $* " == *" -Xclang -ast-dump=json "* ]]; then
  cat "{ast}"
  exit 0
fi
if [[ " $* " == *"_test_cex.cpp"* ]]; then
  cat > "$out" <<'EOF'
#!/usr/bin/env bash
echo CPP_RESULT=0
EOF
  chmod +x "$out"
  exit 0
fi
if [[ " $* " == *" -S "* && " $* " == *" -emit-llvm "* ]]; then
  cat > "$out" <<'EOF'
target triple = "x86_64-unknown-linux-gnu"
define i32 @_Z15ComputeChecksumPKhm(ptr %data, i64 %len) {{
entry:
  ret i32 0
}}
EOF
  exit 0
fi
printf 'BC' > "$out"
"#,
                log = clang_log.display(),
                ast = ast_path.display(),
            ),
        );
        write_script(
            &fake_bin.join("llvm-as"),
            r#"#!/usr/bin/env bash
set -euo pipefail
out=""
prev=""
for arg in "$@"; do
  if [[ "$prev" == "-o" ]]; then out="$arg"; fi
  prev="$arg"
done
printf 'BC' > "$out"
"#,
        );
        write_script(
            &fake_bin.join("llvm-dis"),
            r#"#!/usr/bin/env bash
set -euo pipefail
out=""
prev=""
for arg in "$@"; do
  if [[ "$prev" == "-o" ]]; then out="$arg"; fi
  prev="$arg"
done
cat > "$out" <<'EOF'
target triple = "x86_64-unknown-linux-gnu"
define i32 @_Z15ComputeChecksumPKhm(ptr %data, i64 %len) {
entry:
  ret i32 0
}
EOF
"#,
        );
        write_script(&fake_bin.join("llvm-link"), "#!/usr/bin/env bash\nexit 0\n");
        write_script(
            &fake_bin.join("c++filt"),
            "#!/usr/bin/env bash\necho 'ComputeChecksum(unsigned char const*, unsigned long)'\n",
        );
        let saw_body = match mode {
            SawMode::Verified => {
                "#!/usr/bin/env bash\necho 'VERIFIED'\n"
            }
            SawMode::Disproved => {
                "#!/usr/bin/env bash\nif [[ \"$1\" == \"_eval_cex.saw\" ]]; then echo 'CRYPTOL_RESULT=7'; else printf 'Counterexample\\n  x: 42\\nSubgoal failed: _Z15ComputeChecksumPKhm\\n'; fi\n"
            }
        };
        write_script(&fake_bin.join("saw"), saw_body);
        write_script(&fake_bin.join("z3"), "#!/usr/bin/env bash\nexit 0\n");

        Self {
            root,
            cpp_file,
            cry_file,
            include_dir,
            clang_log,
            fake_bin,
        }
    }

    fn run_verify(&self, out_dir: &Path) -> std::process::ExitStatus {
        self.run_verify_with_args(out_dir, &[])
    }

    fn run_verify_with_args(
        &self,
        out_dir: &Path,
        extra_args: &[&str],
    ) -> std::process::ExitStatus {
        let mut cmd = Command::new(saw_spec_gen_binary());
        cmd.args([
            "verify-cpp",
            "--cpp-file",
            self.cpp_file.to_str().unwrap(),
            "--cryptol-spec",
            self.cry_file.to_str().unwrap(),
            "--cryptol-fn",
            "ComputeChecksum_spec",
            "--function",
            "ComputeChecksum",
            "--output",
            out_dir.to_str().unwrap(),
            "--include-dir",
            self.include_dir.to_str().unwrap(),
            "--cxx-standard",
            "c++20",
            "--clang-flag=-DMOCK=1",
        ])
        .args(extra_args)
        .env("SAW_SPEC_GEN_LLVM_BIN", &self.fake_bin)
        .env("SAW_SPEC_GEN_SAW", self.fake_bin.join("saw"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                self.fake_bin.display(),
                std::env::var("PATH").unwrap()
            ),
        );
        cmd.status().unwrap()
    }
}

/// Integration test for BrokenReason::MsvcMutexHelper (bd issue #65).
///
/// Sets up a fake `verify-cpp` environment where the LLVM IR contains a
/// `linkonce_odr`-defined `std::_Mutex_base` helper function (here
/// `_Verify_ownership_levels`) that performs typed field reads on the mutex
/// struct. Before the fix the extern-override scanner skipped defined
/// non-vararg bodies; the generated `verify.saw` never saw an override for
/// this function and SAW aborted with "Error during memory load" when it
/// tried to inline the typed reads through a symbolically-allocated struct
/// pointer.
///
/// The injected IR uses the real MSVC-mangled name
/// `?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ` so the
/// `_Mutex_base@std` substring pattern fires. This broad pattern future-proofs
/// detection of all sibling `std::_Mutex_base` helpers without a per-method
/// allowlist. The scanner classifies the function as `MsvcMutexHelper` and
/// emits `{{ 1 : [1] }}` (bool true — ownership always valid in a sequential
/// proof) as the pinned no-op return.
#[test]
fn msvc_mutex_helper_emits_noop_override_in_verify_script() {
    let root = TempDir::new("mutex-helper").unwrap();
    let fake_bin = root.path().join("fakebin");
    std::fs::create_dir_all(&fake_bin).unwrap();

    let cpp_file = root.path().join("ownership_check.cpp");
    std::fs::write(
        &cpp_file,
        "extern \"C\" int ComputeChecksum(const unsigned char*, unsigned long) { return 0; }\n",
    )
    .unwrap();
    let cry_file = root.path().join("spec.cry");
    std::fs::write(
        &cry_file,
        "module Main where\nComputeChecksum_spec : [32] -> [32]\nComputeChecksum_spec x = x\n",
    )
    .unwrap();

    let ast_path = repo_root().join("tests/fixtures/request_validator_cpp.json");

    // Fake clang: AST dump → fixture; -S -emit-llvm → IR with the helper;
    // bitcode → empty BC sentinel.  The IR defines ComputeChecksum as a
    // caller of ?_Verify_ownership_levels@_Mutex_base@std@@... (linkonce_odr),
    // using the real MSVC-mangled name so `_Mutex_base@std` fires.
    // Double-brace every `{` / `}` in the IR because this block uses format!.
    write_script(
        &fake_bin.join("clang"),
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
out=""; prev=""
for arg in "$@"; do
  if [[ "$prev" == "-o" ]]; then out="$arg"; fi; prev="$arg"
done
if [[ " $* " == *" -Xclang -ast-dump=json "* ]]; then cat "{ast}"; exit 0; fi
if [[ " $* " == *"_test_cex.cpp"* ]]; then
  echo '#!/usr/bin/env bash' > "$out"; echo 'echo CPP_RESULT=0' >> "$out"; chmod +x "$out"; exit 0
fi
if [[ " $* " == *" -S "* && " $* " == *" -emit-llvm "* ]]; then
  cat > "$out" <<'IREOF'
target triple = "x86_64-pc-windows-msvc"
define i32 @_Z15ComputeChecksumPKhm(ptr %data, i64 %len) {{
entry:
  %self = alloca [8 x i8], align 4
  call i1 @"?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"(ptr %self)
  ret i32 0
}}
define linkonce_odr i1 @"?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"(ptr %0) {{
entry:
  %1 = load i32, ptr %0, align 4
  %2 = getelementptr inbounds i32, ptr %0, i32 1
  %3 = load i32, ptr %2, align 4
  %4 = icmp sle i32 %1, %3
  ret i1 %4
}}
IREOF
  exit 0
fi
printf 'BC' > "$out"
"#,
            ast = ast_path.display(),
        ),
    );
    write_script(
        &fake_bin.join("llvm-as"),
        "#!/usr/bin/env bash\nset -euo pipefail\nout=''; prev=''\nfor arg in \"$@\"; do\n  if [[ \"$prev\" == \"-o\" ]]; then out=\"$arg\"; fi; prev=\"$arg\"\ndone\nprintf 'BC' > \"$out\"\n",
    );
    // llvm-dis returns the same IR (no format! needed — no variable
    // substitution required here, so no escaping of braces).
    write_script(
        &fake_bin.join("llvm-dis"),
        r#"#!/usr/bin/env bash
set -euo pipefail
out=""; prev=""
for arg in "$@"; do
  if [[ "$prev" == "-o" ]]; then out="$arg"; fi; prev="$arg"
done
cat > "$out" <<'IREOF'
target triple = "x86_64-pc-windows-msvc"
define i32 @_Z15ComputeChecksumPKhm(ptr %data, i64 %len) {
entry:
  %self = alloca [8 x i8], align 4
  call i1 @"?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"(ptr %self)
  ret i32 0
}
define linkonce_odr i1 @"?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"(ptr %0) {
entry:
  %1 = load i32, ptr %0, align 4
  %2 = getelementptr inbounds i32, ptr %0, i32 1
  %3 = load i32, ptr %2, align 4
  %4 = icmp sle i32 %1, %3
  ret i1 %4
}
IREOF
"#,
    );
    write_script(&fake_bin.join("llvm-link"), "#!/usr/bin/env bash\nexit 0\n");
    write_script(
        &fake_bin.join("c++filt"),
        "#!/usr/bin/env bash\necho 'ComputeChecksum(unsigned char const*, unsigned long)'\n",
    );
    write_script(
        &fake_bin.join("saw"),
        "#!/usr/bin/env bash\necho 'VERIFIED'\n",
    );
    write_script(&fake_bin.join("z3"), "#!/usr/bin/env bash\nexit 0\n");

    let out_dir = root.path().join("out");
    let status = Command::new(saw_spec_gen_binary())
        .args([
            "verify-cpp",
            "--cpp-file",
            cpp_file.to_str().unwrap(),
            "--cryptol-spec",
            cry_file.to_str().unwrap(),
            "--cryptol-fn",
            "ComputeChecksum_spec",
            "--function",
            "ComputeChecksum",
            "--output",
            out_dir.to_str().unwrap(),
        ])
        .env("SAW_SPEC_GEN_LLVM_BIN", &fake_bin)
        .env("SAW_SPEC_GEN_SAW", fake_bin.join("saw"))
        .env(
            "PATH",
            format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap()),
        )
        .status()
        .unwrap();
    assert!(status.success(), "verify-cpp failed: {status:?}");

    let verify_saw =
        std::fs::read_to_string(out_dir.join("verify.saw")).expect("verify.saw not written");
    assert!(
        verify_saw.contains("[msvc-mutex-helper]"),
        "?_Verify_ownership_levels@_Mutex_base@std@@... must be classified MsvcMutexHelper;\
         \nverify.saw:\n{verify_saw}"
    );
    assert!(
        verify_saw.contains("{{ 1 : [1] }}"),
        "MsvcMutexHelper override must pin {{ 1 : [1] }} (bool true);\
         \nverify.saw:\n{verify_saw}"
    );
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn write_script(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> std::io::Result<Self> {
        let mut p = std::env::temp_dir();
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        p.push(format!("saw-spec-gen-{prefix}-{n}"));
        std::fs::create_dir_all(&p)?;
        Ok(Self(p))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
