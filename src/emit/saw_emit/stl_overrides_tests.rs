use super::*;

#[test]
fn vector_push_back_itanium_matches() {
    // std::vector<uint32_t>::push_back(uint32_t&&)
    assert!(matches_uncached("_ZNSt6vectorIjSaIjEE9push_backEOj"));
}

#[test]
fn vector_back_itanium_matches() {
    // std::vector<uint32_t>::back() const
    assert!(matches_uncached("_ZNKSt6vectorIjSaIjEE4backEv"));
}

#[test]
fn vector_subscript_itanium_matches() {
    // std::vector<uint32_t>::operator[](size_type)
    assert!(matches_uncached("_ZNSt6vectorIjSaIjEEixEm"));
}

#[test]
fn vector_msvc_matches() {
    // Heuristic MSVC mangling fragment for vector<uint32_t>
    assert!(matches_uncached("?back@?$vector@II@std@@QEBA?BIXZ"));
}

#[test]
fn unique_ptr_deref_itanium_matches() {
    // std::unique_ptr<uint32_t>::operator*() const
    assert!(matches_uncached(
        "_ZNKSt10unique_ptrIjSt14default_deleteIjEEdeEv"
    ));
}

#[test]
fn make_unique_itanium_matches() {
    // std::make_unique<uint32_t>(uint32_t&&)
    assert!(matches_uncached(
        "_ZSt11make_uniqueIjJjEENSt9_MakeUniqIT_E15__single_objectEDpOT0_"
    ));
}

#[test]
fn shared_ptr_methods_match() {
    assert!(matches_uncached("_ZNSt10shared_ptrIiE3getEv"));
    assert!(matches_uncached(
        "_ZSt11make_sharedIiJiEESt10shared_ptrIT_EDpOT0_"
    ));
}

#[test]
fn basic_string_methods_match() {
    // std::basic_string<char, ...>::data() const
    assert!(matches_uncached("_ZNKSs4dataEv"));
    // libstdc++ cxx11 ABI flavor
    assert!(matches_uncached(
        "_ZNKSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE4dataEv"
    ));
}

#[test]
fn map_set_unordered_match() {
    assert!(matches_uncached(
        "_ZNSt3mapIiiSt4lessIiESaISt4pairIKiiEEEixERS5_"
    ));
    assert!(matches_uncached(
        "_ZNSt13unordered_mapIiiSt4hashIiESt8equal_toIiESaISt4pairIKiiEEEixERS6_"
    ));
}

#[test]
fn user_code_does_not_match() {
    // mangled name for a top-level user function `add_one(uint32_t)`
    assert!(!matches_uncached("_Z7add_onej"));
    // operator new / delete — these are intentionally NOT in the
    // registry, they have dedicated overrides elsewhere.
    assert!(!matches_uncached("_Znwm"));
    assert!(!matches_uncached("_ZdlPvm"));
    // throw helpers
    assert!(!matches_uncached("_ZSt17__throw_bad_allocv"));
}

#[test]
fn family_for_returns_family_name() {
    assert_eq!(
        family_for("_ZNSt6vectorIjSaIjEE9push_backEOj"),
        Some("std::vector methods")
    );
    assert_eq!(
        family_for("_ZNKSt10unique_ptrIjSt14default_deleteIjEEdeEv"),
        Some("std::unique_ptr methods")
    );
    assert_eq!(family_for("_Z7add_onej"), None);
}

#[test]
fn kill_switch_unaffected_by_uncached() {
    // matches_uncached MUST ignore the env var so that unit tests
    // remain deterministic regardless of developer environment.
    assert!(matches_uncached("_ZNSt6vectorIjSaIjEE9push_backEOj"));
}
