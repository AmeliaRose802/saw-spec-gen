//! Per-parameter `llvm_precond` emitters extracted from
//! [`super::verify_script_steps`] to keep that file under the
//! 500-non-whitespace-line repository limit.

/// Emit the per-parameter `llvm_precond` clauses derived in
/// [`crate::constraints::derive`]. Lines that look like comments are
/// passed through verbatim; everything else gets a trailing `;`.
pub(super) fn emit_param_preconditions(out: &mut String, preconditions: &[String]) {
    for pre in preconditions {
        let trimmed = pre.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("//") {
            out.push_str(&format!("    {pre}\n"));
        } else {
            out.push_str(&format!("    {pre};\n"));
        }
    }
}

/// Like [`emit_param_preconditions`] but drops the auto-emitted
/// "unsized pointer" TODO block (the multi-line warning starting
/// with `// TODO[saw-spec-gen]: pointer parameter \`<name>\` has no
/// length annotation.`). Used in the buffer-override branch where
/// the CLI flag has already supplied the missing size and the
/// warning would only be noise.
///
/// The block layout is owned by
/// [`crate::constraints::length_companion::LengthCompanionGuess::todo_lines`];
/// every continuation line is indented under `//   `, so we drop
/// every consecutive `//   ` line after the marker rather than
/// hard-coding a continuation count.
pub(super) fn emit_param_preconditions_filtered(
    out: &mut String,
    preconditions: &[String],
    param_name: &str,
) {
    let todo_marker = format!(
        "// TODO[saw-spec-gen]: pointer parameter `{param_name}` has no length annotation.",
    );
    let mut iter = preconditions.iter().peekable();
    while let Some(pre) = iter.next() {
        if pre.trim_start() == todo_marker {
            // Skip every consecutive continuation comment line that
            // belongs to this TODO block (they all start with `//   `).
            while let Some(next) = iter.peek() {
                if next.trim_start().starts_with("//   ") {
                    iter.next();
                } else {
                    break;
                }
            }
            continue;
        }
        let trimmed = pre.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("//") {
            out.push_str(&format!("    {pre}\n"));
        } else {
            out.push_str(&format!("    {pre};\n"));
        }
    }
}
