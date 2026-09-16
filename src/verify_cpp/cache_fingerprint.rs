//! Compiler and preprocessed translation-unit identity for the C++ artifact cache.

use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct CompilerStamp {
    executable: PathBuf,
    size: u64,
    modified_before_epoch: bool,
    modified_nanos: u128,
}

#[derive(Serialize)]
struct FingerprintInputs<'a> {
    clang: &'a Path,
    compiler: &'a CompilerStamp,
    target: &'a str,
    user_flags: &'a [String],
    cpp_file: &'a Path,
    working_dir: &'a Path,
}

/// Return a full SHA-256 digest for cache-key flags only, never clang arguments.
/// Preprocessing observes transitive/system includes and environment-selected
/// headers/macros without relying on include-directory timestamp scans.
pub(super) fn fingerprint(
    clang: &Path,
    target: &str,
    user_flags: &[String],
    cpp_file: &Path,
) -> Result<String> {
    let compiler = compiler_stamp(clang)?;
    let working_dir = std::env::current_dir().context("reading compiler working directory")?;
    let version = checked_output(
        Command::new(clang).arg("--version"),
        "clang --version for cache fingerprint",
    )?;
    ensure!(
        !version.stdout.is_empty() || !version.stderr.is_empty(),
        "clang --version returned no compiler identity"
    );
    let preprocessed = checked_output(
        &mut preprocessing_command(clang, target, user_flags, cpp_file)?,
        "clang preprocessing for cache fingerprint",
    )?;
    ensure!(
        compiler == compiler_stamp(clang)?,
        "clang executable changed while computing cache fingerprint; retry verification"
    );
    make_fingerprint(
        &FingerprintInputs {
            clang,
            compiler: &compiler,
            target,
            user_flags,
            cpp_file,
            working_dir: &working_dir,
        },
        &version.stdout,
        &version.stderr,
        &preprocessed.stdout,
    )
}

fn compiler_stamp(clang: &Path) -> Result<CompilerStamp> {
    let executable = clang
        .canonicalize()
        .with_context(|| format!("resolving clang executable {}", clang.display()))?;
    let metadata = fs::metadata(&executable)
        .with_context(|| format!("reading clang executable metadata {}", executable.display()))?;
    ensure!(
        metadata.is_file(),
        "clang executable is not a file: {}",
        executable.display()
    );
    let modified = metadata
        .modified()
        .with_context(|| format!("reading clang modification time {}", executable.display()))?;
    let (modified_before_epoch, elapsed) = match modified.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(elapsed) => (false, elapsed),
        Err(error) => (true, error.duration()),
    };
    // Avoid rereading a large compiler binary; use its resolved path, exact
    // modification time and size together with the separately captured version.
    Ok(CompilerStamp {
        executable,
        size: metadata.len(),
        modified_before_epoch,
        modified_nanos: elapsed.as_nanos(),
    })
}

fn preprocessing_command(
    clang: &Path,
    target: &str,
    user_flags: &[String],
    cpp_file: &Path,
) -> Result<Command> {
    let mut command = Command::new(clang);
    command
        .args(["-E", "-fno-rtti", "-target", target])
        .args(user_flags)
        .arg(clang_source_arg(cpp_file)?)
        .args(["-o", "-"]);
    Ok(command)
}

fn clang_source_arg(path: &Path) -> Result<String> {
    let path = path
        .to_str()
        .context("non-UTF-8 C++ source path for cache fingerprint")?;
    // Match compilation: verbatim Windows paths prevent clang from resolving
    // relative ../ includes through the usual Win32 path normalization.
    Ok(if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        path.to_owned()
    })
}

fn checked_output(command: &mut Command, label: &str) -> Result<Output> {
    let output = command
        .output()
        .with_context(|| format!("running {label}"))?;
    successful_output(output, label)
}

fn successful_output(output: Output, label: &str) -> Result<Output> {
    if !output.status.success() {
        bail!(
            "{label} failed ({}):\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output)
}

fn make_fingerprint(
    inputs: &FingerprintInputs<'_>,
    version_stdout: &[u8],
    version_stderr: &[u8],
    preprocessed: &[u8],
) -> Result<String> {
    // JSON preserves option ordering and boundaries, including embedded spaces.
    let inputs = serde_json::to_vec(inputs).context("serializing compiler fingerprint inputs")?;
    Ok(hash_fields(&[
        b"saw-spec-gen-cpp-cache-fingerprint-v3",
        &inputs,
        version_stdout,
        version_stderr,
        preprocessed,
    ]))
}

fn hash_fields(fields: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for &field in fields {
        hasher.update((field.len() as u64).to_le_bytes());
        // Hash raw stdout, not lossy UTF-8, trimmed text, or a short prefix.
        hasher.update(field);
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = "x86_64-pc-windows-msvc";
    const VERSION: &[u8] = b"clang version test\n";
    const SOURCE: &[u8] = b"# 1 \"header.hpp\"\nstruct Foo { int value; };\n";

    fn compiler() -> CompilerStamp {
        CompilerStamp {
            executable: PathBuf::from("/toolchain/clang"),
            size: 180_000_000,
            modified_before_epoch: false,
            modified_nanos: 1_000,
        }
    }

    fn inputs<'a>(compiler: &'a CompilerStamp, flags: &'a [String]) -> FingerprintInputs<'a> {
        FingerprintInputs {
            clang: Path::new("/toolchain/clang"),
            compiler,
            target: TARGET,
            user_flags: flags,
            cpp_file: Path::new("/project/source.cpp"),
            working_dir: Path::new("/project"),
        }
    }

    fn digest(inputs: &FingerprintInputs<'_>) -> String {
        make_fingerprint(inputs, VERSION, b"", SOURCE).unwrap()
    }

    #[test]
    fn deterministic_full_sha256_with_unambiguous_fields() {
        let compiler = compiler();
        let inputs = inputs(&compiler, &[]);
        let hash = digest(&inputs);
        assert_eq!(hash, digest(&inputs));
        assert_eq!(hash.len(), 64);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(hash, hash.to_ascii_lowercase());
        assert_ne!(hash_fields(&[b"ab", b"c"]), hash_fields(&[b"a", b"bc"]));
        assert_ne!(hash_fields(&[b""]), hash_fields(&[]));
    }

    #[test]
    fn compiler_path_size_and_exact_timestamp_change_the_digest() {
        let original = compiler();
        let expected = digest(&inputs(&original, &[]));
        for changed in [
            CompilerStamp {
                executable: PathBuf::from("/another-toolchain/clang"),
                ..original.clone()
            },
            CompilerStamp {
                size: original.size + 1,
                ..original.clone()
            },
            CompilerStamp {
                modified_nanos: original.modified_nanos + 1,
                ..original.clone()
            },
            CompilerStamp {
                modified_before_epoch: true,
                ..original.clone()
            },
        ] {
            assert_ne!(expected, digest(&inputs(&changed, &[])));
        }
    }

    #[test]
    fn invocation_paths_and_target_change_the_digest() {
        let compiler = compiler();
        let expected = digest(&inputs(&compiler, &[]));
        for changed in [
            FingerprintInputs {
                clang: Path::new("/toolchain/clang++"),
                ..inputs(&compiler, &[])
            },
            FingerprintInputs {
                target: "aarch64-unknown-linux-gnu",
                ..inputs(&compiler, &[])
            },
            FingerprintInputs {
                cpp_file: Path::new("/project/other.cpp"),
                ..inputs(&compiler, &[])
            },
            FingerprintInputs {
                working_dir: Path::new("/other-project"),
                ..inputs(&compiler, &[])
            },
        ] {
            assert_ne!(expected, digest(&changed));
        }
    }

    #[test]
    fn flags_keep_order_boundaries_and_non_preprocessor_options() {
        let compiler = compiler();
        let flags = vec!["-I".into(), "include dir".into(), "-DVALUE=1".into()];
        let expected = digest(&inputs(&compiler, &flags));
        for changed in [
            vec!["-I include dir".into(), "-DVALUE=1".into()],
            vec!["-DVALUE=1".into(), "-I".into(), "include dir".into()],
            vec!["-I".into(), "include dir".into(), "-DVALUE=2".into()],
            [flags.clone(), vec!["-fpack-struct=1".into()]].concat(),
            [flags.clone(), vec!["-O1".into()]].concat(),
        ] {
            assert_ne!(expected, digest(&inputs(&compiler, &changed)));
        }
    }

    #[test]
    fn version_and_all_preprocessed_bytes_change_the_digest() {
        let compiler = compiler();
        let inputs = inputs(&compiler, &[]);
        let expected = digest(&inputs);
        assert_ne!(
            expected,
            make_fingerprint(&inputs, b"clang version newer\n", b"", SOURCE).unwrap()
        );
        assert_ne!(
            expected,
            make_fingerprint(&inputs, VERSION, b"compiler build details", SOURCE).unwrap()
        );
        for changed in [
            b"# 1 \"header.hpp\"\nstruct Foo { long value; };\n".as_slice(),
            b"# 1 \"other/header.hpp\"\nstruct Foo { int value; };\n".as_slice(),
            b"# 1 \"header.hpp\"\nstruct Foo { int value; };\n\n".as_slice(),
        ] {
            let actual = make_fingerprint(&inputs, VERSION, b"", changed).unwrap();
            assert_ne!(expected, actual);
        }
        // These would collide if either compiler output were decoded lossily.
        assert_ne!(
            make_fingerprint(&inputs, VERSION, b"", &[0xff]).unwrap(),
            make_fingerprint(&inputs, VERSION, b"", &[0xfe]).unwrap()
        );
        assert_ne!(
            make_fingerprint(&inputs, &[0xff], b"", SOURCE).unwrap(),
            make_fingerprint(&inputs, &[0xfe], b"", SOURCE).unwrap()
        );
    }

    #[test]
    fn preprocessing_matches_compiler_flags_without_shell_quoting() {
        let clang = Path::new("toolchain with spaces").join("clang");
        let flags = vec!["-I".into(), "include dir".into(), "-O1".into()];
        let command = preprocessing_command(
            &clang,
            TARGET,
            &flags,
            Path::new(r"\\?\C:\source dir\source.cpp"),
        )
        .unwrap();
        assert_eq!(command.get_program(), clang.as_os_str());
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert_eq!(
            args,
            [
                "-E",
                "-fno-rtti",
                "-target",
                TARGET,
                "-I",
                "include dir",
                "-O1",
                r"C:\source dir\source.cpp",
                "-o",
                "-",
            ]
        );
    }

    #[test]
    fn strips_verbatim_drive_and_unc_source_paths() {
        for (path, expected) in [
            (r"\\?\C:\project\source.cpp", r"C:\project\source.cpp"),
            (
                r"\\?\UNC\server\share\source.cpp",
                r"\\server\share\source.cpp",
            ),
            (r"C:\project\source.cpp", r"C:\project\source.cpp"),
            ("/project/source.cpp", "/project/source.cpp"),
        ] {
            assert_eq!(clang_source_arg(Path::new(path)).unwrap(), expected);
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn failed_commands_report_both_output_streams() {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt;

        let output = Output {
            status: std::process::ExitStatus::from_raw(1),
            stdout: b"partial preprocessed output".to_vec(),
            stderr: b"header.hpp: file not found".to_vec(),
        };
        let error = successful_output(output, "clang preprocessing")
            .unwrap_err()
            .to_string();
        assert!(error.contains("clang preprocessing failed ("));
        assert!(error.contains("stdout:\npartial preprocessed output"));
        assert!(error.contains("stderr:\nheader.hpp: file not found"));
    }
}
