// Rust implementation of enforce_access.
// Returns a struct { bool, bool } which lowers to LLVM { i1, i1 }.
// The StructValue bridge decomposes the Cryptol tuple into
// llvm_struct_value [field0, field1].

#[repr(C)]
pub struct AccessResult {
    pub allowed: bool,
    pub logged: bool,
}

#[inline(never)]
pub fn enforce_access(mode: u8, decision: u8) -> AccessResult {
    AccessResult {
        allowed: decision == 1,
        logged: mode == 1 || decision == 0,
    }
}
