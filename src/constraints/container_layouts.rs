//! Container-layout configuration (ArrayView rule 5, saw_spec_gen-26d).
//!
//! Some standard library containers carry their length internally
//! and the spec generator can do better than the default "1-byte
//! fallback alloc" if we know the layout. This module loads a
//! user-supplied TOML file describing such layouts and exposes a
//! lookup API the emitter calls when it encounters a known struct
//! pointee.
//!
//! ## Status
//!
//! This is the MVP scaffold for rule 5: the loader parses the TOML
//! file and surfaces the data, but the emitter only uses the loaded
//! configuration to emit a stderr note ("known container, no
//! lowering yet") on a hit. Auto-emitting `llvm_points_to` chains
//! for the data pointer + size fields is tracked under the same
//! umbrella (saw_spec_gen-rng).
//!
//! ## Default layouts
//!
//! [`builtin_defaults`] returns a small built-in table covering the
//! shapes we see most often in real C++ code. Pass
//! `--container-layouts <path>` to extend or override it.
//!
//! ## TOML schema
//!
//! ```toml
//! [[container]]
//! name      = "std::string"
//! data_ptr  = "_M_dataplus._M_p"   # field path to the data pointer
//! size      = "_M_string_length"   # field path to the size (elements)
//! elem_bits = 8                    # element width in bits
//! ```

use std::collections::HashMap;
use std::path::Path;

/// One known container layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerLayout {
    /// Canonical C++ qualified name (e.g. `"std::string"`,
    /// `"std::vector"`). Matched against `TypeInfo::Struct` names
    /// modulo a small set of normalizations (template arg stripping,
    /// `__cxx11::` namespace).
    pub name: String,
    /// Field path from the container's outermost struct to the
    /// element-data pointer (e.g. `"_M_dataplus._M_p"`).
    pub data_ptr: String,
    /// Field path from the container's outermost struct to the
    /// element-count integer (e.g. `"_M_string_length"`).
    pub size: String,
    /// Width of one element in bits (8 for `char`, 32 for `int`,
    /// etc.).
    pub elem_bits: u32,
}

/// A loaded container-layout catalog. The lookup key is the
/// canonical container name; the value is the layout.
#[derive(Debug, Clone, Default)]
pub struct ContainerCatalog {
    by_name: HashMap<String, ContainerLayout>,
}

impl ContainerCatalog {
    /// Construct a catalog seeded with [`builtin_defaults`].
    pub fn with_defaults() -> Self {
        let mut c = Self::default();
        for layout in builtin_defaults() {
            c.by_name.insert(layout.name.clone(), layout);
        }
        c
    }

    /// Merge layouts from a TOML file. Returns the number of layouts
    /// loaded. Newer entries replace older ones with the same name.
    pub fn load_toml_file(&mut self, path: &Path) -> std::io::Result<usize> {
        let text = std::fs::read_to_string(path)?;
        let count = self
            .load_toml_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(count)
    }

    /// Parse TOML text directly. Returns the number of layouts
    /// loaded, or an error string describing the parse failure.
    pub fn load_toml_str(&mut self, text: &str) -> Result<usize, String> {
        // Hand-rolled parse to avoid pulling in a new dependency for
        // the scaffold. We only support the documented shape:
        //
        //   [[container]]
        //   name      = "..."
        //   data_ptr  = "..."
        //   size      = "..."
        //   elem_bits = N
        //
        // Anything else is rejected so we don't silently accept typos.
        let mut count = 0;
        let mut current: Option<PartialLayout> = None;
        for (lineno, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line == "[[container]]" {
                if let Some(p) = current.take() {
                    let l = p.finish(lineno)?;
                    self.by_name.insert(l.name.clone(), l);
                    count += 1;
                }
                current = Some(PartialLayout::default());
                continue;
            }
            let cur = current.as_mut().ok_or_else(|| {
                format!("line {}: key without preceding [[container]]", lineno + 1)
            })?;
            cur.set_key_value(line, lineno)?;
        }
        if let Some(p) = current.take() {
            let l = p.finish(text.lines().count())?;
            self.by_name.insert(l.name.clone(), l);
            count += 1;
        }
        Ok(count)
    }

    /// Look up a container layout by its canonical name. Returns
    /// `None` when the name is unknown.
    pub fn lookup(&self, name: &str) -> Option<&ContainerLayout> {
        self.by_name.get(name)
    }

    /// Insert (or replace) a single layout. Used by the AST auto-derive
    /// path in [`super::container_layouts_derive::derive_catalog_from_structs`]
    /// and by the optional TOML loader.
    pub fn insert(&mut self, layout: ContainerLayout) {
        self.by_name.insert(layout.name.clone(), layout);
    }

    /// Merge every entry from `other` into `self`. Later entries
    /// override earlier ones with the same name.
    pub fn extend_from(&mut self, other: ContainerCatalog) {
        for (_, layout) in other.by_name {
            self.insert(layout);
        }
    }

    /// Total layout count (for diagnostics).
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// True iff no layouts are registered.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

#[derive(Debug, Default)]
struct PartialLayout {
    name: Option<String>,
    data_ptr: Option<String>,
    size: Option<String>,
    elem_bits: Option<u32>,
}

impl PartialLayout {
    fn set_key_value(&mut self, line: &str, lineno: usize) -> Result<(), String> {
        let (k, v) = line.split_once('=').ok_or_else(|| {
            format!(
                "line {}: expected `key = value`, got `{}`",
                lineno + 1,
                line
            )
        })?;
        let key = k.trim();
        let val = v.trim();
        match key {
            "name" => self.name = Some(strip_quotes(val, lineno)?.to_string()),
            "data_ptr" => self.data_ptr = Some(strip_quotes(val, lineno)?.to_string()),
            "size" => self.size = Some(strip_quotes(val, lineno)?.to_string()),
            "elem_bits" => {
                self.elem_bits = Some(val.parse::<u32>().map_err(|_| {
                    format!("line {}: elem_bits must be a positive integer", lineno + 1)
                })?);
            }
            other => {
                return Err(format!("line {}: unknown key `{}`", lineno + 1, other));
            }
        }
        Ok(())
    }

    fn finish(self, lineno: usize) -> Result<ContainerLayout, String> {
        Ok(ContainerLayout {
            name: self
                .name
                .ok_or_else(|| format!("line {}: [[container]] missing `name`", lineno + 1))?,
            data_ptr: self
                .data_ptr
                .ok_or_else(|| format!("line {}: [[container]] missing `data_ptr`", lineno + 1))?,
            size: self
                .size
                .ok_or_else(|| format!("line {}: [[container]] missing `size`", lineno + 1))?,
            elem_bits: self
                .elem_bits
                .ok_or_else(|| format!("line {}: [[container]] missing `elem_bits`", lineno + 1))?,
        })
    }
}

fn strip_quotes(s: &str, lineno: usize) -> Result<&str, String> {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) && s.len() >= 2 {
        Ok(&s[1..s.len() - 1])
    } else {
        Err(format!(
            "line {}: expected a double-quoted string, got `{}`",
            lineno + 1,
            s
        ))
    }
}

/// Built-in default catalog. Keep small — the file in
/// `config/container_layouts.toml` is the canonical extension point.
pub fn builtin_defaults() -> Vec<ContainerLayout> {
    vec![
        // libstdc++ std::string (libstdc++ ABI). Field paths are the
        // typical short-string-optimized layout exposed by libstdc++
        // 5+ (`std::__cxx11::basic_string<char>`); the catalog uses
        // the dropped-namespace name for callsite convenience.
        ContainerLayout {
            name: "std::string".into(),
            data_ptr: "_M_dataplus._M_p".into(),
            size: "_M_string_length".into(),
            elem_bits: 8,
        },
        // libstdc++ std::vector<T> (any T): three-pointer layout
        // (`_M_start`, `_M_finish`, `_M_end_of_storage`). We expose
        // `_M_start` as the data pointer; `size` is computed as
        // `(_M_finish - _M_start)` by the emitter — for the scaffold
        // we just publish `_M_finish` and let the emitter handle the
        // subtraction.
        ContainerLayout {
            name: "std::vector".into(),
            data_ptr: "_M_start".into(),
            size: "_M_finish".into(),
            elem_bits: 0, // element-size unknown at catalog time
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_include_std_string_and_vector() {
        let c = ContainerCatalog::with_defaults();
        assert!(c.lookup("std::string").is_some());
        assert!(c.lookup("std::vector").is_some());
    }

    #[test]
    fn parses_single_container_block() {
        let mut c = ContainerCatalog::default();
        let n = c
            .load_toml_str(
                r#"
                [[container]]
                name      = "my::Buffer"
                data_ptr  = "data"
                size      = "len"
                elem_bits = 8
                "#,
            )
            .unwrap();
        assert_eq!(n, 1);
        let layout = c.lookup("my::Buffer").unwrap();
        assert_eq!(layout.data_ptr, "data");
        assert_eq!(layout.size, "len");
        assert_eq!(layout.elem_bits, 8);
    }

    #[test]
    fn rejects_unknown_keys() {
        let mut c = ContainerCatalog::default();
        let err = c
            .load_toml_str(
                r#"
                [[container]]
                name      = "X"
                data_ptr  = "p"
                size      = "n"
                elem_bits = 8
                bogus_key = "oops"
                "#,
            )
            .unwrap_err();
        assert!(err.contains("unknown key `bogus_key`"), "{err}");
    }

    #[test]
    fn rejects_missing_required_key() {
        let mut c = ContainerCatalog::default();
        let err = c
            .load_toml_str(
                r#"
                [[container]]
                name      = "X"
                data_ptr  = "p"
                size      = "n"
                "#,
            )
            .unwrap_err();
        assert!(err.contains("missing `elem_bits`"), "{err}");
    }

    #[test]
    fn parses_multiple_containers() {
        let mut c = ContainerCatalog::default();
        let n = c
            .load_toml_str(
                r#"
                [[container]]
                name      = "A"
                data_ptr  = "d"
                size      = "s"
                elem_bits = 8

                [[container]]
                name      = "B"
                data_ptr  = "d"
                size      = "s"
                elem_bits = 16
                "#,
            )
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn later_entry_overrides_earlier_one() {
        let mut c = ContainerCatalog::with_defaults();
        let before = c.lookup("std::string").unwrap().elem_bits;
        assert_eq!(before, 8);
        c.load_toml_str(
            r#"
            [[container]]
            name      = "std::string"
            data_ptr  = "_M_dataplus._M_p"
            size      = "_M_string_length"
            elem_bits = 16
            "#,
        )
        .unwrap();
        assert_eq!(c.lookup("std::string").unwrap().elem_bits, 16);
    }

    #[test]
    fn rejects_unquoted_string_value() {
        let mut c = ContainerCatalog::default();
        let err = c
            .load_toml_str(
                r#"
                [[container]]
                name      = unquoted
                data_ptr  = "p"
                size      = "n"
                elem_bits = 8
                "#,
            )
            .unwrap_err();
        assert!(err.contains("double-quoted"), "{err}");
    }
}
