//! Generates/refreshes `svtav1-rs/COVERAGE.md` — the coverage-gate
//! scoreboard, auto-derived from the C API surface so no field can be
//! silently omitted.
//!
//! Parses `Source/API/EbSvtAv1Enc.h`'s `EbSvtAv1EncConfiguration` struct and
//! emits one row per field. Status values are hand-maintained
//! (`unmapped` -> `mapped` -> `tested:<test-name>`) and are PRESERVED across
//! regenerations by field name; new upstream fields appear as `unmapped`,
//! removed fields drop out. The coverage gate is green when every row is
//! `tested`.
//!
//! Usage (from svtav1-rs/):
//!   cargo run --release -p svtav1-cref --bin gen_coverage

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.ancestors().nth(3).unwrap().to_path_buf();
    let header = repo_root.join("Source/API/EbSvtAv1Enc.h");
    let coverage_path = repo_root.join("svtav1-rs/COVERAGE.md");

    let src = std::fs::read_to_string(&header).expect("read EbSvtAv1Enc.h");

    // Extract the struct block.
    let start = src
        .find("typedef struct EbSvtAv1EncConfiguration")
        .expect("struct start");
    let end = src[start..]
        .find("} EbSvtAv1EncConfiguration;")
        .expect("struct end")
        + start;
    let block = &src[start..end];

    // Parse fields: lines ending in `;` that declare a member.
    // Keep the last doc-comment line seen before the field as its hint.
    let mut fields: Vec<(String, String, String)> = Vec::new(); // (name, type, hint)
    let mut last_comment = String::new();
    for line in block.lines() {
        let t = line.trim();
        if t.starts_with("/*") || t.starts_with("*") || t.starts_with("//") {
            let c = t
                .trim_start_matches("/**")
                .trim_start_matches("/*!")
                .trim_start_matches("/*")
                .trim_start_matches("*/")
                .trim_start_matches("*")
                .trim_start_matches("//")
                .trim();
            if !c.is_empty() && !c.starts_with('@') {
                last_comment = c.trim_end_matches("*/").trim().to_string();
            }
            continue;
        }
        if !t.ends_with(';') || t.starts_with('#') || t.contains("typedef") {
            continue;
        }
        let decl = t.trim_end_matches(';');
        // Split off the field name (last identifier, may carry array dims).
        let Some((ty, name)) = decl.rsplit_once(|c: char| c.is_whitespace()) else {
            continue;
        };
        let name = name.trim_start_matches('*');
        let (name, dims) = match name.find('[') {
            Some(i) => (&name[..i], &name[i..]),
            None => (name, ""),
        };
        if name.is_empty() {
            continue;
        }
        fields.push((
            name.to_string(),
            format!("{}{}", ty.split_whitespace().collect::<Vec<_>>().join(" "), dims),
            std::mem::take(&mut last_comment),
        ));
    }

    // Preserve existing statuses by field name.
    let mut statuses: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(old) = std::fs::read_to_string(&coverage_path) {
        for line in old.lines() {
            let cols: Vec<&str> = line.split('|').map(str::trim).collect();
            // | field | type | status | notes |
            if cols.len() >= 4 && !cols[1].is_empty() && cols[1] != "field" && !cols[1].starts_with('-') {
                statuses.insert(cols[1].trim_matches('`').to_string(), cols[3].to_string());
            }
        }
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Coverage gate — EbSvtAv1EncConfiguration surface\n\n\
         Auto-derived from `Source/API/EbSvtAv1Enc.h` by `gen_coverage` (do not\n\
         edit the field list by hand — rerun the generator after baseline\n\
         bumps). Statuses ARE hand-maintained and survive regeneration:\n\
         `unmapped` -> `mapped` (plumbed through the Rust config) ->\n\
         `tested:<test>` (a passing test exercises it against the gates).\n\
         The coverage gate is green when every row is `tested`.\n"
    );
    let total = fields.len();
    let counted = |pfx: &str| {
        fields
            .iter()
            .filter(|(n, _, _)| statuses.get(n).map(|s| s.starts_with(pfx)).unwrap_or(false))
            .count()
    };
    let _ = writeln!(
        out,
        "**{total} fields** — tested: {}, mapped: {}, unmapped: {}\n",
        counted("tested"),
        counted("mapped"),
        total - counted("tested") - counted("mapped"),
    );
    let _ = writeln!(out, "| field | type | status | notes |");
    let _ = writeln!(out, "|---|---|---|---|");
    for (name, ty, hint) in &fields {
        let status = statuses.get(name).cloned().unwrap_or_else(|| "unmapped".into());
        let _ = writeln!(out, "| `{name}` | `{ty}` | {status} | {hint} |");
    }

    std::fs::write(&coverage_path, &out).expect("write COVERAGE.md");
    eprintln!("wrote {} fields to {}", total, coverage_path.display());
}
