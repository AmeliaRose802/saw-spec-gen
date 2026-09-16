//! Global-write attribution shared by the legacy and typed override scans.

use std::collections::{HashMap, HashSet, VecDeque};

use super::{IrFunc, MutableGlobals};

/// Walk every defined body reachable from `start` (through `call`/
/// `invoke` edges), unioning the bare global symbol names that those
/// bodies directly `store` into. When the walk reaches a DeclareOnly
/// callee, conservatively union **all externally-visible** mutable
/// globals — the opaque body could be a forward-declared function
/// from another project TU that `extern`s any such symbol and writes
/// it. Globals with `internal`/`private` linkage are excluded since
/// other TUs cannot reference them.
pub(super) fn collect_globals_written_from(
    start: &str,
    by_name: &HashMap<&str, &IrFunc>,
    mg: &MutableGlobals,
) -> Vec<String> {
    let mut written: HashSet<String> = HashSet::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut worklist: VecDeque<String> = VecDeque::new();
    worklist.push_back(start.to_string());
    while let Some(s) = worklist.pop_front() {
        if !visited.insert(s.clone()) {
            continue;
        }
        let Some(f) = by_name.get(s.as_str()) else {
            // Not even declared in the module — skip.
            continue;
        };
        if !f.is_define {
            // LLVM intrinsics are compiler-implemented primitives,
            // not real external functions — they cannot access user
            // globals.
            if s.starts_with("llvm.") {
                continue;
            }
            // Opaque callee — could be a forward-declared function
            // from another project TU that writes any externally-
            // visible global.
            written.extend(mg.externally_visible.iter().cloned());
            continue;
        }
        for g in &f.body_globals_stored {
            if mg.all.contains(g.as_str()) {
                written.insert(g.clone());
            }
        }
        for callee in &f.body_calls {
            if !visited.contains(callee.as_str()) {
                worklist.push_back(callee.clone());
            }
        }
    }
    let mut out: Vec<String> = written.into_iter().collect();
    out.sort();
    out
}
