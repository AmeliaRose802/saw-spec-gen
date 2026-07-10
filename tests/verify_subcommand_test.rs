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

#[test]
fn verify_subcommand_loop_invariant_config_emits_fixpoint_chc() {
    // Verify that declaring loop_invariants in the sibling .toml config:
    //   1. Switches verify.saw from llvm_verify to llvm_verify_fixpoint_chc
    //   2. Adds a proof_mode comment in verify.saw
    //   3. Writes proof_mode: "invariant" in result.json
    let env = FakeVerifyEnv::new(SawMode::Verified);
    let out_dir = env.root.path().join("out_invariant");
    std::fs::write(
        env.cry_file.with_extension("toml"),
        r#"[functions.ComputeChecksum_spec]
loop_invariants = ["checksum_loop_inv"]
"#,
    )
    .unwrap();

    let status = env.run_verify_with_args(&out_dir, &[]);
    assert!(status.success(), "verify failed: {status:?}");

    let saw = std::fs::read_to_string(out_dir.join("verify.saw")).unwrap();
    assert!(
        saw.contains("llvm_verify_fixpoint_chc"),
        "expected llvm_verify_fixpoint_chc in invariant mode; got:\n{saw}"
    );
    assert!(
        saw.contains("// proof_mode: invariant"),
        "expected proof_mode comment in verify.saw; got:\n{saw}"
    );
    assert!(
        saw.contains("// Loop invariants declared:"),
        "expected invariant list comment; got:\n{saw}"
    );
    assert!(
        saw.contains("//   - checksum_loop_inv"),
        "expected invariant name in comment; got:\n{saw}"
    );
    assert!(
        !saw.contains("llvm_verify m "),
        "should NOT use plain llvm_verify in invariant mode; got:\n{saw}"
    );

    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("result.json")).unwrap())
            .unwrap();
    assert_eq!(result["proof_mode"], "invariant");
}

#[test]
fn verify_subcommand_default_mode_is_bounded() {
    // Without loop_invariants, the generated script uses plain llvm_verify
    // and result.json has proof_mode: "bounded".
    let env = FakeVerifyEnv::new(SawMode::Verified);
    let out_dir = env.root.path().join("out_bounded");

    let status = env.run_verify_with_args(&out_dir, &[]);
    assert!(status.success(), "verify failed: {status:?}");

    let saw = std::fs::read_to_string(out_dir.join("verify.saw")).unwrap();
    assert!(
        saw.contains("llvm_verify m "),
        "expected plain llvm_verify in bounded mode; got:\n{saw}"
    );
    assert!(
        !saw.contains("llvm_verify_fixpoint_chc"),
        "should NOT use fixpoint_chc in bounded mode; got:\n{saw}"
    );

    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("result.json")).unwrap())
            .unwrap();
    assert_eq!(result["proof_mode"], "bounded");
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
